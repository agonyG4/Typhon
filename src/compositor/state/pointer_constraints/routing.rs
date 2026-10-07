use super::*;

impl CompositorState {
    pub(in crate::compositor) fn effective_locked_pointer_surface_for_absolute_motion(
        &self,
    ) -> Option<u32> {
        self.pointer_constraint_runtime
            .locked_surface_for_absolute_motion()
    }

    pub(in crate::compositor) fn cursor_hidden_by_pointer_lock(&self) -> bool {
        self.cursor_visibility.lock_hidden_constraint_id.is_some()
    }

    pub(in crate::compositor) fn active_locked_pointer_binding(
        &self,
    ) -> Option<ActiveLockedPointerRouting> {
        let active = self
            .pointer_constraint_runtime
            .active_locked_pointer_routing
            .as_ref()?;
        let constraint = self
            .pointer_constraint_runtime
            .constraints
            .get(&active.constraint_id)?;
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
        self.pointer_constraint_runtime
            .active_locked_pointer_routing = None;
        self.invalidate_locked_relative_recipient_cache();
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
        let active = self
            .pointer_constraint_runtime
            .active_confined_pointer_routing
            .as_ref()?;
        let constraint = self
            .pointer_constraint_runtime
            .constraints
            .get(&active.constraint_id)?;
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
        self.pointer_constraint_runtime
            .active_confined_pointer_routing = None;
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
        self.pointer_constraint_runtime
            .active_confined_pointer_routing = Some(ActiveConfinedPointerRouting {
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
}
