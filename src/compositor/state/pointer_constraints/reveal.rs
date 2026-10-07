use super::*;

impl CompositorState {
    pub(in crate::compositor) fn pointer_constraint_reveal_authority(
        &self,
    ) -> Option<CursorRevealAuthority> {
        self.pointer_constraint_runtime
            .last_cursor_reveal_authority()
    }

    pub(in crate::compositor) fn complete_pointer_constraint_reveal_trace_if(
        &mut self,
        completed: CursorRevealAuthority,
    ) {
        self.pointer_constraint_runtime
            .clear_cursor_reveal_authority_if(completed);
    }

    #[cfg(test)]
    pub(in crate::compositor) fn has_pending_locked_pointer_reveal(&self) -> bool {
        self.pointer_constraint_runtime.has_pending_locked_reveal()
    }

    #[cfg(test)]
    #[cfg(test)]
    pub(in crate::compositor) fn active_locked_pointer_anchor_for_test(
        &self,
    ) -> Option<(f64, f64)> {
        self.pointer_constraint_runtime
            .active_locked_pointer_anchor()
            .map(|position| (position.x, position.y))
    }

    pub(in crate::compositor) fn begin_client_dispatch_cycle(&mut self) {
        self.pointer_constraint_runtime.dispatch_epoch = self
            .pointer_constraint_runtime
            .dispatch_epoch
            .saturating_add(1);
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
        fallback_origin: Option<PointerWarpOrigin>,
    ) {
        if crate::pointer_debug::cursor_presentation_trace_enabled() {
            if let Some(previous) = self
                .pointer_constraint_runtime
                .last_cursor_reveal_authority
                .filter(|authority| authority.visibility_requested)
            {
                crate::pointer_debug::cursor_presentation_log_lazy(|| {
                    format!(
                        "event=cursor_reveal_terminal reason=superseded_by_new_reveal constraint={}/{} visibility_requested={} final_position=({},{})",
                        previous.constraint.constraint_id,
                        previous.constraint.generation,
                        previous.visibility_requested,
                        previous.final_position.x,
                        previous.final_position.y
                    )
                });
            }
            self.pointer_constraint_runtime.last_cursor_reveal_authority = None;
        } else {
            self.pointer_constraint_runtime.last_cursor_reveal_authority = None;
        }
        pointer_debug_log(format!(
            "pointer.unlock transition_begin id={} generation={} fallback=({}) epoch={} cursor_kept_hidden=true",
            backend_id.constraint_id,
            backend_id.generation,
            fallback_position
                .map(|position| format!("{},{}", position.x, position.y))
                .unwrap_or_else(|| "none".to_string()),
            self.pointer_constraint_runtime.dispatch_epoch
        ));
        crate::pointer_debug::cursor_presentation_log_lazy(|| {
            format!(
                "event=unlock_reveal_begin constraint={}/{} dispatch_epoch={} fallback_position={} fallback=({},{}) fallback_origin={} pointer_position=({},{}) lock_hidden_constraint={}",
                backend_id.constraint_id,
                backend_id.generation,
                self.pointer_constraint_runtime.dispatch_epoch,
                fallback_position.is_some(),
                fallback_position.map_or(0.0, |position| position.x),
                fallback_position.map_or(0.0, |position| position.y),
                fallback_origin.map_or("none", PointerWarpOrigin::as_str),
                self.last_pointer_x,
                self.last_pointer_y,
                self.cursor_visibility
                    .lock_hidden_constraint_id
                    .map_or_else(|| "none".to_string(), |id| id.to_string())
            )
        });
        self.pointer_constraint_runtime
            .pending_locked_pointer_reveal = Some(PendingLockedPointerReveal {
            backend_id,
            pointer,
            surface,
            fallback_position,
            fallback_origin,
            backend_restore_settled: false,
            backend_settled_dispatch_epoch: None,
            client_warp_position: None,
        });
    }

    pub(in crate::compositor) fn cancel_pending_locked_pointer_reveal_for_id(
        &mut self,
        id: PointerConstraintBackendId,
        reason: &str,
    ) {
        if self
            .pointer_constraint_runtime
            .pending_locked_pointer_reveal
            .as_ref()
            .is_some_and(|pending| pending.backend_id == id)
        {
            pointer_debug_log(format!(
                "pointer.unlock transition_cancel id={} generation={} reason={}",
                id.constraint_id, id.generation, reason
            ));
            self.pointer_constraint_runtime
                .pending_locked_pointer_reveal = None;
        }
    }

    pub(in crate::compositor) fn cancel_pending_locked_pointer_reveal_for_constraint(
        &mut self,
        constraint_id: u64,
        reason: &str,
    ) {
        if self
            .pointer_constraint_runtime
            .pending_locked_pointer_reveal
            .as_ref()
            .is_some_and(|pending| pending.backend_id.constraint_id == constraint_id)
        {
            pointer_debug_log(format!(
                "pointer.unlock transition_cancel id={} reason={}",
                constraint_id, reason
            ));
            self.pointer_constraint_runtime
                .pending_locked_pointer_reveal = None;
        }
    }

    pub(in crate::compositor) fn pending_locked_pointer_reveal_matches(
        &self,
        pointer: &wl_pointer::WlPointer,
        surface: &wl_surface::WlSurface,
    ) -> bool {
        self.pointer_constraint_runtime
            .pending_locked_pointer_reveal
            .as_ref()
            .is_some_and(|pending| {
                same_wayland_resource(&pending.pointer, pointer)
                    && pending.surface.id().same_client_as(&surface.id())
            })
    }

    pub(in crate::compositor) fn mark_pending_locked_pointer_backend_settled(
        &mut self,
        id: PointerConstraintBackendId,
    ) {
        let Some(pending) = self
            .pointer_constraint_runtime
            .pending_locked_pointer_reveal
            .as_mut()
            .filter(|pending| pending.backend_id == id)
        else {
            return;
        };
        pending.backend_restore_settled = true;
        pending.backend_settled_dispatch_epoch =
            Some(self.pointer_constraint_runtime.dispatch_epoch);
        pointer_debug_log(format!(
            "pointer.unlock backend_restore_settled id={} generation={} epoch={}",
            id.constraint_id, id.generation, self.pointer_constraint_runtime.dispatch_epoch
        ));
        crate::pointer_debug::cursor_presentation_log_lazy(|| {
            format!(
                "event=unlock_backend_settled constraint={}/{} dispatch_epoch={}",
                id.constraint_id, id.generation, self.pointer_constraint_runtime.dispatch_epoch
            )
        });
        self.try_settle_pending_locked_pointer_reveal("backend_restore_settled");
    }

    pub(in crate::compositor) fn record_pending_locked_pointer_client_warp(
        &mut self,
        requested_position: OutputPosition,
        accepted_position: OutputPosition,
        origin: PointerWarpOrigin,
    ) {
        let Some(pending) = self
            .pointer_constraint_runtime
            .pending_locked_pointer_reveal
            .as_mut()
        else {
            return;
        };
        pending.client_warp_position = Some(accepted_position);
        pointer_debug_log(format!(
            "pointer.unlock client_warp_position=({}, {}) id={} generation={}",
            accepted_position.x,
            accepted_position.y,
            pending.backend_id.constraint_id,
            pending.backend_id.generation
        ));
        let backend_id = pending.backend_id;
        crate::pointer_debug::cursor_presentation_log_lazy(|| {
            format!(
                "event=unlock_client_warp_observed constraint={}/{} warp_origin={} requested=({},{}) accepted=({},{})",
                backend_id.constraint_id,
                backend_id.generation,
                origin.as_str(),
                requested_position.x,
                requested_position.y,
                accepted_position.x,
                accepted_position.y
            )
        });
    }

    pub(in crate::compositor) fn try_settle_pending_locked_pointer_reveal(&mut self, reason: &str) {
        let should_finalize = self
            .pointer_constraint_runtime
            .pending_locked_pointer_reveal
            .as_ref()
            .is_some_and(|pending| {
                pending.backend_restore_settled
                    && pending.client_warp_position.is_some()
                    && pending
                        .backend_settled_dispatch_epoch
                        .is_some_and(|settled_epoch| {
                            settled_epoch.saturating_add(1)
                                < self.pointer_constraint_runtime.dispatch_epoch
                        })
            });
        if should_finalize {
            self.finalize_pending_locked_pointer_reveal(reason);
        }
    }

    fn settle_pending_locked_pointer_reveal_fallback(&mut self) {
        let Some(pending) = self
            .pointer_constraint_runtime
            .pending_locked_pointer_reveal
            .as_ref()
        else {
            return;
        };
        let Some(position) = pending.fallback_position else {
            self.finalize_pending_locked_pointer_reveal("dispatch_cycle_fallback");
            return;
        };
        let Some(origin) = pending.fallback_origin else {
            self.finalize_pending_locked_pointer_reveal("dispatch_cycle_fallback_without_origin");
            return;
        };
        self.deliver_pointer_reposition(position, origin);
        self.finalize_pending_locked_pointer_reveal("dispatch_cycle_fallback");
    }

    pub(in crate::compositor) fn finalize_pending_locked_pointer_reveal(&mut self, reason: &str) {
        let Some(pending) = self
            .pointer_constraint_runtime
            .pending_locked_pointer_reveal
            .take()
        else {
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
        let final_position = pending
            .client_warp_position
            .or(pending.fallback_position)
            .unwrap_or(OutputPosition {
                x: self.last_pointer_x,
                y: self.last_pointer_y,
            });
        let visibility_requested = self.cursor_visibility.theme_fallback_visible();
        if crate::pointer_debug::cursor_presentation_trace_enabled() {
            self.pointer_constraint_runtime.last_cursor_reveal_authority =
                Some(CursorRevealAuthority {
                    constraint: pending.backend_id,
                    final_position,
                    visibility_requested,
                });
            if !visibility_requested {
                crate::pointer_debug::cursor_presentation_log_lazy(|| {
                    format!(
                        "event=cursor_reveal_terminal reason=no_visible_cursor_requested constraint={}/{} visibility_requested=false final_position=({},{})",
                        pending.backend_id.constraint_id,
                        pending.backend_id.generation,
                        final_position.x,
                        final_position.y
                    )
                });
                self.pointer_constraint_runtime.last_cursor_reveal_authority = None;
            }
        }
        pointer_debug_log(format!(
            "pointer.unlock transition_finalize reason={} id={} generation={} final=({},{}) visibility_request={} epoch={}",
            reason,
            pending.backend_id.constraint_id,
            pending.backend_id.generation,
            final_position.x,
            final_position.y,
            self.cursor_visibility.theme_fallback_visible(),
            self.pointer_constraint_runtime.dispatch_epoch
        ));
        crate::pointer_debug::cursor_presentation_log_lazy(|| {
            format!(
                "event=unlock_reveal_finalize reason={} constraint={}/{} client_warp=({}) fallback=({}) final=({},{}) cursor_visibility_requested={} dispatch_epoch={}",
                reason,
                pending.backend_id.constraint_id,
                pending.backend_id.generation,
                pending.client_warp_position.map_or_else(
                    || "none".to_string(),
                    |position| format!("{},{}", position.x, position.y)
                ),
                pending.fallback_position.map_or_else(
                    || "none".to_string(),
                    |position| format!("{},{}", position.x, position.y)
                ),
                final_position.x,
                final_position.y,
                visibility_requested,
                self.pointer_constraint_runtime.dispatch_epoch
            )
        });
        self.sync_cursor_visibility_request();
    }

    pub(in crate::compositor) fn finalize_pending_locked_pointer_reveal_after_dispatch(&mut self) {
        let should_finalize = self
            .pointer_constraint_runtime
            .pending_locked_pointer_reveal
            .as_ref()
            .is_some_and(|pending| {
                let Some(settled_epoch) = pending.backend_settled_dispatch_epoch else {
                    return false;
                };
                if pending.client_warp_position.is_some() {
                    settled_epoch.saturating_add(1) < self.pointer_constraint_runtime.dispatch_epoch
                } else {
                    settled_epoch.saturating_add(2) < self.pointer_constraint_runtime.dispatch_epoch
                }
            });
        if should_finalize {
            if self
                .pointer_constraint_runtime
                .pending_locked_pointer_reveal
                .as_ref()
                .is_some_and(|pending| pending.client_warp_position.is_none())
            {
                self.settle_pending_locked_pointer_reveal_fallback();
            } else {
                self.finalize_pending_locked_pointer_reveal("dispatch_grace_elapsed");
            }
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
        requested: OutputPosition,
        origin: PointerWarpOrigin,
    ) -> Option<OutputPosition> {
        if self.active_locked_pointer_binding().is_some() {
            pointer_debug_log("pointer warp ignored reason=active_lock");
            crate::pointer_debug::cursor_presentation_log_lazy(|| {
                format!(
                    "event=pointer_warp origin={} requested=({},{}) accepted=false applied=false reason=active_lock",
                    origin.as_str(),
                    requested.x,
                    requested.y
                )
            });
            return None;
        }
        let constraint = if let Some(active) = self.active_confined_pointer_binding() {
            let final_position = active.region.closest_point(requested);
            pointer_debug_log(format!(
                "pointer.reposition request origin={} constraint=confined requested=({},{}) final=({},{})",
                origin.as_str(),
                requested.x,
                requested.y,
                final_position.x,
                final_position.y
            ));
            final_position
        } else {
            pointer_debug_log(format!(
                "pointer.reposition request origin={} constraint=none requested=({},{}) final=({},{})",
                origin.as_str(),
                requested.x,
                requested.y,
                requested.x,
                requested.y
            ));
            requested
        };
        let before = OutputPosition {
            x: self.last_pointer_x,
            y: self.last_pointer_y,
        };
        self.update_pointer_position(constraint.x, constraint.y);
        pointer_debug_log(format!(
            "pointer warp compositor before=({},{}) after=({},{}) origin={}",
            before.x,
            before.y,
            constraint.x,
            constraint.y,
            origin.as_str()
        ));
        crate::pointer_debug::cursor_presentation_log_lazy(|| {
            format!(
                "event=pointer_warp origin={} requested=({},{}) accepted=({},{}) applied=true pending_reveal={}",
                origin.as_str(),
                requested.x,
                requested.y,
                constraint.x,
                constraint.y,
                self.pointer_constraint_runtime
                    .pending_locked_pointer_reveal
                    .is_some()
            )
        });
        self.pending_pointer_constraint_backend_requests.push(
            PointerConstraintBackendRequest::WarpPointer {
                position: constraint,
                origin,
            },
        );
        self.deliver_pointer_reposition(constraint, origin);
        Some(constraint)
    }

    pub(in crate::compositor) fn deliver_pointer_reposition(
        &mut self,
        position: OutputPosition,
        origin: PointerWarpOrigin,
    ) {
        if self.wayland_pointer_dnd_routing_active() {
            return;
        }
        if self.active_locked_pointer_binding().is_some() {
            pointer_debug_log(format!(
                "pointer.reposition delivery suppressed origin={} reason=active_lock",
                origin.as_str()
            ));
            return;
        }
        if let Some(active) = self.active_confined_pointer_binding() {
            self.pin_confined_pointer_focus(&active);
            let Some(target) =
                self.pointer_target_for_surface_at_output(&active.surface, position.x, position.y)
            else {
                pointer_debug_log(format!(
                    "pointer.reposition delivery dropped origin={} reason=confined_local_unresolved",
                    origin.as_str()
                ));
                return;
            };
            self.send_pointer_enter_if_needed(&target);
            self.send_pointer_reposition_to_resources(&target, origin);
            return;
        }
        if self.send_implicit_pointer_grab_reposition(position.x, position.y, origin) {
            pointer_debug_log(format!(
                "pointer.reposition delivery origin={} focus=implicit_grab",
                origin.as_str()
            ));
            return;
        }
        let Some(target) = self.pointer_target_at(position.x, position.y) else {
            self.clear_pointer_focus();
            return;
        };
        if !self.pointer_target_allowed_by_popup_grab(&target) {
            self.clear_pointer_focus();
            return;
        }
        let focus_changed = !self
            .pointer_surface
            .as_ref()
            .is_some_and(|surface| same_surface_resource(surface, &target.surface));
        self.ensure_pointer_focus(&target.surface);
        if focus_changed {
            self.send_pointer_enter_if_needed(&target);
            pointer_debug_log(format!(
                "pointer.reposition delivery origin={} focus=crossing event=enter",
                origin.as_str()
            ));
            return;
        }
        self.send_pointer_enter_if_needed(&target);
        self.send_pointer_reposition_to_resources(&target, origin);
    }

    fn send_pointer_reposition_to_resources(
        &mut self,
        target: &PointerTarget,
        origin: PointerWarpOrigin,
    ) {
        let time = wayland_event_time();
        for pointer in self
            .pointer_resources
            .iter()
            .filter(|pointer| resource_belongs_to_surface_client(*pointer, &target.surface))
        {
            if !self.pointer_resource_entered_surface(pointer, &target.surface) {
                continue;
            }
            if matches!(origin, PointerWarpOrigin::LockedPointerCursorHint)
                && pointer.version() < WL_POINTER_WARP_SINCE
            {
                continue;
            }
            let event = if pointer.version() >= WL_POINTER_WARP_SINCE {
                pointer_debug_log(format!(
                    "pointer.reposition delivery origin={} focus=same pointer={} version={} event=warp local=({},{})",
                    origin.as_str(),
                    pointer.id().protocol_id(),
                    pointer.version(),
                    target.surface_x,
                    target.surface_y
                ));
                wl_pointer::Event::Warp {
                    surface_x: target.surface_x,
                    surface_y: target.surface_y,
                }
            } else {
                pointer_debug_log(format!(
                    "pointer.reposition delivery origin={} focus=same pointer={} version={} event=legacy_motion local=({},{})",
                    origin.as_str(),
                    pointer.id().protocol_id(),
                    pointer.version(),
                    target.surface_x,
                    target.surface_y
                ));
                wl_pointer::Event::Motion {
                    time,
                    surface_x: target.surface_x,
                    surface_y: target.surface_y,
                }
            };
            let _ = pointer.send_event(event);
            send_pointer_frame_if_supported(pointer);
        }
    }

    fn send_implicit_pointer_grab_reposition(
        &mut self,
        x: f64,
        y: f64,
        origin: PointerWarpOrigin,
    ) -> bool {
        let Some(surface) = self.implicit_pointer_grab_surface("surface-destroyed") else {
            return false;
        };
        let Some(target) = self.pointer_target_for_grabbed_surface_at_output(&surface, x, y) else {
            let surface_id = compositor_surface_id(&surface);
            self.cancel_implicit_pointer_grab_for_surface_ids(&[surface_id], "surface-destroyed");
            self.refresh_pointer_focus_at_last_position();
            return true;
        };
        pointer_debug_log(format!(
            "pointer.reposition delivery origin={} focus=implicit_grab surface={} local=({},{})",
            origin.as_str(),
            compositor_surface_id(&surface),
            target.surface_x,
            target.surface_y
        ));
        self.send_pointer_reposition_to_resources(&target, origin);
        true
    }
}
