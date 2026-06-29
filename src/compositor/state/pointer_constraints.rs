use super::*;

impl CompositorState {
    #[cfg(test)]
    pub(in crate::compositor) fn activate_pointer_constraint_for_focused_surface(
        &mut self,
        mode: PointerConstraintMode,
    ) -> bool {
        let Some(surface) = self.pointer_surface.as_ref() else {
            return false;
        };
        self.pointer_constraint
            .activate(mode, compositor_surface_id(surface));
        true
    }

    pub(in crate::compositor) fn clear_pointer_constraint(&mut self) {
        self.pointer_constraint.clear();
    }

    pub(in crate::compositor) fn sync_cursor_visibility_request(&mut self) {
        let desired_visible = self.cursor_visibility.desired_visible();
        if self.cursor_visibility.visible == desired_visible {
            return;
        }
        self.cursor_visibility.visible = desired_visible;
        pointer_debug_log(format!(
            "cursor visibility effective visible={} client_hidden={} lock_hidden={:?}",
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

    pub(in crate::compositor) fn begin_client_dispatch_cycle(&mut self) {
        self.dispatch_epoch = self.dispatch_epoch.saturating_add(1);
    }

    pub(in crate::compositor) fn finish_client_dispatch_cycle(&mut self) {
        self.finalize_pending_locked_pointer_reveal_after_dispatch();
    }

    pub(in crate::compositor) fn begin_pending_locked_pointer_reveal(
        &mut self,
        backend_id: PointerConstraintBackendId,
        pointer: wl_pointer::WlPointer,
        surface: wl_surface::WlSurface,
        fallback_position: Option<OutputPosition>,
    ) {
        pointer_debug_log(format!(
            "pointer.unlock transition_begin id={} generation={} fallback=({}) epoch={} cursor_kept_hidden=true",
            backend_id.constraint_id,
            backend_id.generation,
            fallback_position
                .map(|position| format!("{},{}", position.x, position.y))
                .unwrap_or_else(|| "none".to_string()),
            self.dispatch_epoch
        ));
        self.pending_locked_pointer_reveal = Some(PendingLockedPointerReveal {
            backend_id,
            pointer,
            surface,
            fallback_position,
            created_dispatch_epoch: self.dispatch_epoch,
        });
    }

    pub(in crate::compositor) fn cancel_pending_locked_pointer_reveal_for_id(
        &mut self,
        id: PointerConstraintBackendId,
        reason: &str,
    ) {
        if self
            .pending_locked_pointer_reveal
            .as_ref()
            .is_some_and(|pending| pending.backend_id == id)
        {
            pointer_debug_log(format!(
                "pointer.unlock transition_cancel id={} generation={} reason={}",
                id.constraint_id, id.generation, reason
            ));
            self.pending_locked_pointer_reveal = None;
        }
    }

    pub(in crate::compositor) fn cancel_pending_locked_pointer_reveal_for_constraint(
        &mut self,
        constraint_id: u64,
        reason: &str,
    ) {
        if self
            .pending_locked_pointer_reveal
            .as_ref()
            .is_some_and(|pending| pending.backend_id.constraint_id == constraint_id)
        {
            pointer_debug_log(format!(
                "pointer.unlock transition_cancel id={} reason={}",
                constraint_id, reason
            ));
            self.pending_locked_pointer_reveal = None;
        }
    }

    pub(in crate::compositor) fn pending_locked_pointer_reveal_matches(
        &self,
        pointer: &wl_pointer::WlPointer,
        surface: &wl_surface::WlSurface,
    ) -> bool {
        self.pending_locked_pointer_reveal
            .as_ref()
            .is_some_and(|pending| {
                same_wayland_resource(&pending.pointer, pointer)
                    && same_surface_resource(&pending.surface, surface)
            })
    }

    pub(in crate::compositor) fn finalize_pending_locked_pointer_reveal(&mut self, reason: &str) {
        let Some(pending) = self.pending_locked_pointer_reveal.take() else {
            return;
        };
        if self.cursor_visibility.lock_hidden_constraint_id
            == Some(pending.backend_id.constraint_id)
        {
            self.cursor_visibility.lock_hidden_constraint_id = None;
            if self.active_client_cursor_has_content() {
                self.advance_render_generation(RenderGenerationCause::CursorState);
            }
        }
        pointer_debug_log(format!(
            "pointer.unlock transition_finalize reason={} id={} generation={} final=({}) visibility_request={} epoch={}",
            reason,
            pending.backend_id.constraint_id,
            pending.backend_id.generation,
            pending
                .fallback_position
                .map(|position| format!("{},{}", position.x, position.y))
                .unwrap_or_else(|| format!("{},{}", self.last_pointer_x, self.last_pointer_y)),
            self.cursor_visibility.desired_visible(),
            self.dispatch_epoch
        ));
        self.sync_cursor_visibility_request();
    }

    pub(in crate::compositor) fn finalize_pending_locked_pointer_reveal_after_dispatch(&mut self) {
        let should_finalize = self
            .pending_locked_pointer_reveal
            .as_ref()
            .is_some_and(|pending| {
                pending.created_dispatch_epoch.saturating_add(2) < self.dispatch_epoch
            });
        if should_finalize {
            self.finalize_pending_locked_pointer_reveal("dispatch_cycle_fallback");
        }
    }

    pub(in crate::compositor) fn allocate_internal_pointer_constraint_id(&mut self) -> u64 {
        self.next_internal_pointer_constraint_id = self
            .next_internal_pointer_constraint_id
            .saturating_add(1)
            .max(1);
        self.next_internal_pointer_constraint_id
    }

    pub(in crate::compositor) fn active_locked_pointer_binding(
        &self,
    ) -> Option<ActiveLockedPointerRouting> {
        let active = self.active_locked_pointer_routing.as_ref()?;
        let constraint = self.pointer_constraints.get(&active.constraint_id)?;
        if constraint.generation != active.generation
            || !constraint.active
            || constraint.defunct
            || constraint.mode != PointerConstraintMode::Locked
        {
            return None;
        }
        if !active.pointer.is_alive() || !active.surface.is_alive() {
            return None;
        }
        Some(active.clone())
    }

    pub(in crate::compositor) fn clear_active_locked_pointer_routing(&mut self) {
        self.active_locked_pointer_routing = None;
    }

    pub(in crate::compositor) fn pin_locked_pointer_focus(
        &mut self,
        active: &ActiveLockedPointerRouting,
    ) {
        self.ensure_pointer_focus(&active.surface);
        if !self.pointer_resource_entered_surface(&active.pointer, &active.surface) {
            let target = PointerTarget {
                surface: active.surface.clone(),
                surface_x: active.surface_x,
                surface_y: active.surface_y,
            };
            self.send_pointer_enter_to_resource(&active.pointer, &target);
        }
    }

    pub(in crate::compositor) fn locked_pointer_input_surface(
        &self,
    ) -> Option<wl_surface::WlSurface> {
        self.active_locked_pointer_binding()
            .map(|active| active.surface)
    }

    pub(in crate::compositor) fn active_confined_pointer_binding(
        &self,
    ) -> Option<ActiveConfinedPointerRouting> {
        let active = self.active_confined_pointer_routing.as_ref()?;
        let constraint = self.pointer_constraints.get(&active.constraint_id)?;
        if constraint.generation != active.generation
            || !constraint.active
            || constraint.defunct
            || constraint.mode != PointerConstraintMode::Confined
        {
            return None;
        }
        if !active.pointer.is_alive() || !active.surface.is_alive() {
            return None;
        }
        Some(active.clone())
    }

    pub(in crate::compositor) fn clear_active_confined_pointer_routing(&mut self) {
        self.active_confined_pointer_routing = None;
    }

    pub(in crate::compositor) fn pin_confined_pointer_focus(
        &mut self,
        active: &ActiveConfinedPointerRouting,
    ) {
        if !self
            .pointer_surface
            .as_ref()
            .is_some_and(|current| same_surface_resource(current, &active.surface))
        {
            self.pointer_surface = Some(active.surface.clone());
        }
        if !self.pointer_resource_entered_surface(&active.pointer, &active.surface) {
            let target = self
                .pointer_target_for_surface_at_output(
                    &active.surface,
                    self.last_pointer_x,
                    self.last_pointer_y,
                )
                .unwrap_or(PointerTarget {
                    surface: active.surface.clone(),
                    surface_x: 0.0,
                    surface_y: 0.0,
                });
            self.send_pointer_enter_to_resource(&active.pointer, &target);
        }
    }

    pub(in crate::compositor) fn send_confined_pointer_motion(&mut self, x: f64, y: f64) {
        let Some(active) = self.active_confined_pointer_binding() else {
            return;
        };
        let proposed = OutputPosition { x, y };
        let clamped = active.region.closest_point(proposed);
        self.update_pointer_position(clamped.x, clamped.y);
        self.pin_confined_pointer_focus(&active);
        let Some(target) =
            self.pointer_target_for_surface_at_output(&active.surface, clamped.x, clamped.y)
        else {
            pointer_debug_log(format!(
                "confined motion dropped id={} reason=local_unresolved proposed=({},{}) clamped=({},{})",
                active.constraint_id, x, y, clamped.x, clamped.y
            ));
            return;
        };
        pointer_debug_log(format!(
            "confined motion proposed=({},{}) clamped=({},{}) surface_local=({},{})",
            x, y, clamped.x, clamped.y, target.surface_x, target.surface_y
        ));
        let time = wayland_event_time();
        for pointer in self
            .pointer_resources
            .iter()
            .filter(|pointer| resource_belongs_to_surface_client(*pointer, &active.surface))
        {
            let _ = pointer.send_event(wl_pointer::Event::Motion {
                time,
                surface_x: target.surface_x,
                surface_y: target.surface_y,
            });
            send_pointer_frame_if_supported(pointer);
        }
    }

    pub(in crate::compositor) fn register_pointer_constraint(
        &mut self,
        registration: PointerConstraintRegistration,
    ) -> bool {
        if let Some(existing) = self.pointer_constraints.values().find(|constraint| {
            !constraint.defunct && same_surface_resource(&constraint.surface, &registration.surface)
        }) {
            pointer_debug_log(format!(
                "constraint reject already_constrained existing={} requested={} surface={} pointer={}",
                existing.id,
                registration.id,
                compositor_surface_id(&registration.surface),
                registration.pointer.id().protocol_id()
            ));
            return false;
        }

        self.next_pointer_constraint_generation = self
            .next_pointer_constraint_generation
            .wrapping_add(1)
            .max(1);
        let generation = self.next_pointer_constraint_generation;
        pointer_debug_log(format!(
            "constraint create id={} generation={} mode={:?} surface={} pointer={} client={}",
            registration.id,
            generation,
            registration.mode,
            compositor_surface_id(&registration.surface),
            registration.pointer.id().protocol_id(),
            wayland_resource_client_label(&registration.pointer)
        ));
        self.pointer_constraints.insert(
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
                defunct: false,
                pending_region: registration.region.clone(),
                committed_region: registration.region,
                pending_cursor_position_hint: None,
                committed_cursor_position_hint: None,
            },
        );
        self.maybe_request_pointer_constraint_activation(registration.id);
        true
    }

    pub(in crate::compositor) fn maybe_request_pointer_constraint_activation(
        &mut self,
        constraint_id: u64,
    ) {
        let Some((pointer, surface)) =
            self.pointer_constraints
                .get(&constraint_id)
                .and_then(|constraint| {
                    if constraint.active || constraint.backend_pending || constraint.defunct {
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
            || self.root_surface_id_for_surface(compositor_surface_id(&focused))
                != self.root_surface_id_for_surface(compositor_surface_id(&surface))
        {
            pointer_debug_log(format!(
                "pointer.constraint activation deferred id={} reason=focus_client_or_root_mismatch focused={} owner={}",
                constraint_id,
                compositor_surface_id(&focused),
                compositor_surface_id(&surface)
            ));
            return;
        }
        if self.active_backend_constraint.is_some() || self.pending_backend_constraint.is_some() {
            pointer_debug_log(format!(
                "backend activate requested id={} skipped current_active={:?} current_pending={:?}",
                constraint_id, self.active_backend_constraint, self.pending_backend_constraint
            ));
            return;
        }
        let confinement_region = self.pointer_constraint_output_region(constraint_id);
        let request = {
            let Some(constraint) = self.pointer_constraints.get(&constraint_id) else {
                return;
            };
            let backend_id = constraint.backend_id();
            match constraint.mode {
                PointerConstraintMode::Locked => {
                    let Some(anchor) = self.pointer_constraint_activation_anchor(
                        constraint_id,
                        confinement_region.as_ref(),
                    ) else {
                        pointer_debug_log(format!(
                            "constraint activation skipped id={} reason=anchor_unresolved",
                            constraint.id
                        ));
                        return;
                    };
                    let target = self
                        .pointer_target_for_grabbed_surface_at_output(&surface, anchor.x, anchor.y)
                        .unwrap_or(PointerTarget {
                            surface: surface.clone(),
                            surface_x: anchor.x,
                            surface_y: anchor.y,
                        });
                    self.ensure_pointer_focus(&surface);
                    self.send_pointer_enter_to_resource(&pointer, &target);
                    PointerConstraintBackendRequest::ActivateLocked {
                        id: backend_id,
                        anchor,
                    }
                }
                PointerConstraintMode::Confined => {
                    let Some(region) = confinement_region else {
                        pointer_debug_log(format!(
                            "constraint activation skipped id={} reason=region_unresolved mode={:?}",
                            constraint.id, constraint.mode
                        ));
                        return;
                    };
                    PointerConstraintBackendRequest::ActivateConfined {
                        id: backend_id,
                        region,
                    }
                }
                PointerConstraintMode::None => PointerConstraintBackendRequest::Deactivate {
                    id: backend_id,
                    restore_position: None,
                },
            }
        };
        let Some(constraint) = self.pointer_constraints.get_mut(&constraint_id) else {
            return;
        };
        if constraint.active || constraint.backend_pending || constraint.defunct {
            return;
        }
        let backend_id = constraint.backend_id();
        self.pending_backend_constraint = Some(backend_id);
        constraint.backend_pending = true;
        if let PointerConstraintBackendRequest::ActivateLocked { anchor, .. } = &request {
            self.pending_locked_activation_anchors
                .insert(backend_id, *anchor);
        } else {
            self.pending_locked_activation_anchors.remove(&backend_id);
        }
        pointer_debug_log(format!(
            "constraint activation queued id={} generation={}",
            backend_id.constraint_id, backend_id.generation
        ));
        self.pending_pointer_constraint_backend_requests
            .push(request);
    }

    pub(in crate::compositor) fn pointer_constraint_activation_anchor(
        &self,
        constraint_id: u64,
        region: Option<&OutputRegion>,
    ) -> Option<OutputPosition> {
        let constraint = self.pointer_constraints.get(&constraint_id)?;
        let current = OutputPosition {
            x: self.last_pointer_x,
            y: self.last_pointer_y,
        };
        let Some(region) = region else {
            return Some(current);
        };
        if region.closest_point(current) == current {
            return Some(current);
        }
        let owner_root =
            self.root_surface_id_for_surface(compositor_surface_id(&constraint.surface));
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
        let (surface_id, constraint_region, surface_resource) = self
            .pointer_constraints
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
            .map(|data| {
                let mut rows = Vec::new();
                for y in 0..renderable.height {
                    let mut run_start = None;
                    for x in 0..renderable.width {
                        let surface_x = f64::from(x);
                        let surface_y = f64::from(y);
                        let contained = constraint_region.contains(
                            surface_x,
                            surface_y,
                            renderable.width,
                            renderable.height,
                        ) && data.input_region_contains(
                            surface_x,
                            surface_y,
                            renderable.width,
                            renderable.height,
                        );
                        match (run_start, contained) {
                            (None, true) => run_start = Some(x),
                            (Some(start), false) => {
                                if let Some(rect) = OutputRect::new(
                                    f64::from(origin.0 + start as i32),
                                    f64::from(origin.1 + y as i32),
                                    f64::from(x - start),
                                    1.0,
                                ) {
                                    rows.push(rect);
                                }
                                run_start = None;
                            }
                            _ => {}
                        }
                    }
                    if let Some(start) = run_start
                        && let Some(rect) = OutputRect::new(
                            f64::from(origin.0 + start as i32),
                            f64::from(origin.1 + y as i32),
                            f64::from(renderable.width - start),
                            1.0,
                        )
                    {
                        rows.push(rect);
                    }
                }
                rows
            })
            .unwrap_or_default();
        if input_region.is_empty() {
            None
        } else {
            Some(OutputRegion {
                rects: coalesce_output_row_rects(input_region),
            })
        }
    }

    pub(in crate::compositor) fn pointer_constraint_backend_activated(
        &mut self,
        id: PointerConstraintBackendId,
    ) {
        if self.pending_backend_constraint != Some(id) {
            pointer_debug_log(format!(
                "backend activated stale id={:?} current_active={:?} current_pending={:?}",
                id, self.active_backend_constraint, self.pending_backend_constraint
            ));
            self.pending_locked_activation_anchors.remove(&id);
            return;
        }
        let locked_activation_anchor = self.pending_locked_activation_anchors.remove(&id);
        let activation = {
            let Some(constraint) = self.pointer_constraints.get_mut(&id.constraint_id) else {
                return;
            };
            if constraint.generation != id.generation || constraint.defunct {
                return;
            }
            constraint.backend_pending = false;
            if constraint.active {
                return;
            }
            constraint.active = true;
            self.pending_backend_constraint = None;
            self.active_backend_constraint = Some(id);
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
        self.pointer_constraint.activate(mode, surface_id);
        if mode == PointerConstraintMode::Locked {
            if let Some(pending) = self.pending_locked_pointer_reveal.take() {
                pointer_debug_log(format!(
                    "pointer.unlock transition_cancel id={} generation={} reason=new_lock",
                    pending.backend_id.constraint_id, pending.backend_id.generation
                ));
            }
            let activation_anchor = locked_activation_anchor.unwrap_or(OutputPosition {
                x: self.last_pointer_x,
                y: self.last_pointer_y,
            });
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
            self.active_locked_pointer_routing = Some(ActiveLockedPointerRouting {
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
                self.active_confined_pointer_routing = Some(ActiveConfinedPointerRouting {
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
                if let Some(resource) = &locked_resource {
                    resource.locked();
                }
            }
            PointerConstraintMode::Confined => {
                if let Some(resource) = &confined_resource {
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
        self.pending_backend_constraint == Some(id)
            && self
                .pointer_constraints
                .get(&id.constraint_id)
                .is_some_and(|constraint| {
                    constraint.generation == id.generation
                        && constraint.backend_pending
                        && !constraint.active
                        && !constraint.defunct
                })
    }

    pub(in crate::compositor) fn pointer_constraint_backend_failed(
        &mut self,
        id: PointerConstraintBackendId,
        _reason: &str,
    ) {
        if self.pending_backend_constraint == Some(id) {
            self.pending_backend_constraint = None;
        }
        self.pending_locked_activation_anchors.remove(&id);
        self.cancel_pending_locked_pointer_reveal_for_id(id, "backend_failed");
        let Some(constraint) = self.pointer_constraints.get_mut(&id.constraint_id) else {
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
        if self.active_backend_constraint == Some(id) {
            self.active_backend_constraint = None;
        }
        self.deactivate_pointer_constraint_by_id(id.constraint_id, true, true, false);
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
        self.pending_locked_activation_anchors.remove(&id);
        self.cancel_pending_locked_pointer_reveal_for_id(id, "constraint_backend_work_canceled");
        if self.pending_backend_constraint == Some(id) {
            self.pending_backend_constraint = None;
        }
    }

    pub(in crate::compositor) fn deactivate_pointer_constraint_by_id(
        &mut self,
        constraint_id: u64,
        compositor_driven: bool,
        emit_event: bool,
        queue_backend_deactivate: bool,
    ) {
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
            let Some(constraint) = self.pointer_constraints.get_mut(&constraint_id) else {
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
        } else if self.pending_backend_constraint == Some(backend_id) {
            self.pending_backend_constraint = None;
        }
        if self.active_backend_constraint == Some(backend_id) {
            self.active_backend_constraint = None;
        }
        let restore_position = if self
            .active_locked_pointer_routing
            .as_ref()
            .is_some_and(|active| active.constraint_id == constraint_id)
        {
            let restore_position = if mode == PointerConstraintMode::Locked {
                self.restore_locked_pointer_position(&surface, cursor_position_hint)
            } else {
                None
            };
            if mode == PointerConstraintMode::Locked {
                self.clear_active_locked_pointer_routing();
                self.refresh_pointer_focus_at_last_position();
            }
            restore_position
        } else if self
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
        if was_active {
            self.clear_pointer_constraint();
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
                    },
                );
            }
            if emit_event {
                match mode {
                    PointerConstraintMode::Locked => {
                        if let Some(resource) = &locked_resource {
                            resource.unlocked();
                        }
                    }
                    PointerConstraintMode::Confined => {
                        if let Some(resource) = &confined_resource {
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
                );
            }
        } else if was_pending {
            pointer_debug_log(format!(
                "constraint pending activation canceled id={} generation={}",
                backend_id.constraint_id, backend_id.generation
            ));
            if mode == PointerConstraintMode::Locked
                && lifetime == PointerConstraintLifetime::Oneshot
                && let Some(position) =
                    self.valid_cursor_hint_output_position(&surface, cursor_position_hint)
            {
                pointer_debug_log(format!(
                    "oneshot compatibility warp selected id={} generation={} output=({},{})",
                    backend_id.constraint_id, backend_id.generation, position.x, position.y
                ));
                self.apply_pointer_warp(position, true);
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

    pub(in crate::compositor) fn valid_cursor_hint_output_position(
        &mut self,
        surface: &wl_surface::WlSurface,
        cursor_position_hint: Option<(f64, f64)>,
    ) -> Option<OutputPosition> {
        let (surface_x, surface_y) = cursor_position_hint?;
        if !surface_x.is_finite() || !surface_y.is_finite() {
            pointer_debug_log(format!(
                "pointer cursor_hint ignored reason=non_finite hint=({},{})",
                surface_x, surface_y
            ));
            return None;
        }
        let (x, y) = self.output_position_for_valid_cursor_hint(surface, surface_x, surface_y)?;
        Some(OutputPosition { x, y })
    }

    pub(in crate::compositor) fn apply_pointer_warp(
        &mut self,
        position: OutputPosition,
        send_motion: bool,
    ) {
        let before = OutputPosition {
            x: self.last_pointer_x,
            y: self.last_pointer_y,
        };
        self.update_pointer_position(position.x, position.y);
        pointer_debug_log(format!(
            "pointer warp compositor before=({},{}) after=({},{}) send_motion={}",
            before.x, before.y, position.x, position.y, send_motion
        ));
        self.pending_pointer_constraint_backend_requests
            .push(PointerConstraintBackendRequest::WarpPointer { position });
        if send_motion {
            self.send_pointer_motion_after_warp(position);
        }
    }

    pub(in crate::compositor) fn send_pointer_motion_after_warp(
        &mut self,
        position: OutputPosition,
    ) {
        if self.active_locked_pointer_binding().is_some() {
            pointer_debug_log("pointer warp motion suppressed reason=active_lock");
            return;
        }
        if let Some(active) = self.active_confined_pointer_binding() {
            self.pin_confined_pointer_focus(&active);
            return;
        }
        if self.send_implicit_pointer_grab_motion(position.x, position.y) {
            return;
        }
        let Some(target) = self.pointer_target_at(position.x, position.y) else {
            self.clear_pointer_focus();
            return;
        };
        self.ensure_pointer_focus(&target.surface);
        let time = wayland_event_time();
        for pointer in self
            .pointer_resources
            .iter()
            .filter(|pointer| resource_belongs_to_surface_client(*pointer, &target.surface))
        {
            let _ = pointer.send_event(wl_pointer::Event::Motion {
                time,
                surface_x: target.surface_x,
                surface_y: target.surface_y,
            });
            send_pointer_frame_if_supported(pointer);
        }
    }

    pub(in crate::compositor) fn remove_pointer_constraint(&mut self, constraint_id: u64) {
        self.cancel_pending_locked_pointer_reveal_for_constraint(
            constraint_id,
            "constraint_removed",
        );
        let was_active = self
            .pointer_constraints
            .get(&constraint_id)
            .is_some_and(|constraint| constraint.active || constraint.backend_pending);
        if was_active {
            if let Some(constraint) = self.pointer_constraints.get_mut(&constraint_id) {
                constraint.defunct = true;
            }
            self.deactivate_pointer_constraint_by_id(constraint_id, false, false, true);
        }
        self.pointer_constraints.remove(&constraint_id);
        if self.cursor_visibility.lock_hidden_constraint_id == Some(constraint_id)
            && self
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

    pub(in crate::compositor) fn deactivate_pointer_constraints_for_pointer(
        &mut self,
        pointer: &wl_pointer::WlPointer,
        emit_event: bool,
    ) {
        let ids = self
            .pointer_constraints
            .values()
            .filter(|constraint| same_wayland_resource(&constraint.pointer, pointer))
            .map(|constraint| constraint.id)
            .collect::<Vec<_>>();
        for id in ids {
            self.cancel_pending_locked_pointer_reveal_for_constraint(id, "pointer_destroyed");
            if let Some(constraint) = self.pointer_constraints.get_mut(&id) {
                constraint.defunct = true;
            }
            self.deactivate_pointer_constraint_by_id(id, true, emit_event, true);
            self.pointer_constraints.remove(&id);
        }
    }

    pub(in crate::compositor) fn deactivate_pointer_constraints_for_surface(
        &mut self,
        surface_id: u32,
        emit_event: bool,
    ) {
        let ids = self
            .pointer_constraints
            .values()
            .filter(|constraint| compositor_surface_id(&constraint.surface) == surface_id)
            .map(|constraint| constraint.id)
            .collect::<Vec<_>>();
        for id in ids {
            self.cancel_pending_locked_pointer_reveal_for_constraint(id, "surface_destroyed");
            if let Some(constraint) = self.pointer_constraints.get_mut(&id) {
                constraint.defunct = true;
            }
            self.deactivate_pointer_constraint_by_id(id, true, emit_event, true);
            self.pointer_constraints.remove(&id);
        }
    }

    pub(in crate::compositor) fn deactivate_pointer_constraints_for_surface_focus_loss(
        &mut self,
        surface_id: u32,
        emit_event: bool,
    ) {
        let ids = self
            .pointer_constraints
            .values()
            .filter(|constraint| compositor_surface_id(&constraint.surface) == surface_id)
            .map(|constraint| constraint.id)
            .collect::<Vec<_>>();
        for id in ids {
            self.deactivate_pointer_constraint_by_id(id, true, emit_event, true);
        }
    }

    pub(in crate::compositor) fn set_pointer_constraint_pending_region(
        &mut self,
        constraint_id: u64,
        region: SurfaceInputRegion,
    ) {
        if let Some(constraint) = self.pointer_constraints.get_mut(&constraint_id) {
            constraint.pending_region = region;
        }
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
        if let Some(constraint) = self.pointer_constraints.get_mut(&constraint_id) {
            constraint.pending_cursor_position_hint = Some((surface_x, surface_y));
        }
    }

    pub(in crate::compositor) fn apply_pending_pointer_constraint_state_for_surface(
        &mut self,
        surface_id: u32,
    ) {
        let ids = self
            .pointer_constraints
            .values()
            .filter(|constraint| compositor_surface_id(&constraint.surface) == surface_id)
            .map(|constraint| constraint.id)
            .collect::<Vec<_>>();
        for id in ids {
            if let Some(constraint) = self.pointer_constraints.get_mut(&id) {
                constraint.committed_region = constraint.pending_region.clone();
                constraint.committed_cursor_position_hint = constraint.pending_cursor_position_hint;
            }
            self.update_active_confined_pointer_region(id, "commit");
            self.maybe_request_pointer_constraint_activation(id);
        }
    }

    pub(in crate::compositor) fn update_active_confined_pointer_region(
        &mut self,
        constraint_id: u64,
        reason: &'static str,
    ) {
        let Some(active) = self.active_confined_pointer_binding() else {
            return;
        };
        if active.constraint_id != constraint_id {
            return;
        }
        let Some(region) = self.pointer_constraint_output_region(constraint_id) else {
            return;
        };
        if region == active.region {
            return;
        }
        pointer_debug_log(format!(
            "confined route update id={} old={:?} new={:?} reason={}",
            constraint_id, active.region.rects, region.rects, reason
        ));
        let id = PointerConstraintBackendId {
            constraint_id,
            generation: active.generation,
        };
        self.pending_pointer_constraint_backend_requests.push(
            PointerConstraintBackendRequest::UpdateConfinedRegion {
                id,
                region: region.clone(),
            },
        );
        self.active_confined_pointer_routing = Some(ActiveConfinedPointerRouting {
            region: region.clone(),
            ..active
        });
        let position = OutputPosition {
            x: self.last_pointer_x,
            y: self.last_pointer_y,
        };
        if region.closest_point(position) != position {
            self.send_confined_pointer_motion(position.x, position.y);
        }
    }

    pub(in crate::compositor) fn update_all_active_confined_pointer_regions(
        &mut self,
        reason: &'static str,
    ) {
        let Some(active) = self.active_confined_pointer_binding() else {
            return;
        };
        self.update_active_confined_pointer_region(active.constraint_id, reason);
    }

    pub(in crate::compositor) fn take_pointer_constraint_backend_requests(
        &mut self,
    ) -> Vec<PointerConstraintBackendRequest> {
        std::mem::take(&mut self.pending_pointer_constraint_backend_requests)
    }
}
