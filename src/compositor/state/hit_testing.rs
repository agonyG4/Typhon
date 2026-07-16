use super::*;

impl CompositorState {
    pub(in crate::compositor) fn surface_id_at(&mut self, x: f64, y: f64) -> Option<u32> {
        self.refresh_surface_origin_cache();
        let origins = &self.surface_origin_cache;
        for (index, renderable) in self.renderable_surfaces.iter().enumerate().rev() {
            let Some(origin) = origins.get(index).copied() else {
                continue;
            };
            let Some((surface_x, surface_y)) =
                render::surface_local_point_at_origin(renderable, origin, x, y)
            else {
                continue;
            };
            if self.surface_accepts_input_at(renderable, surface_x, surface_y) {
                return Some(renderable.surface_id);
            }
        }

        None
    }

    pub(in crate::compositor) fn root_surface_hit_at(
        &mut self,
        x: f64,
        y: f64,
    ) -> Option<RootSurfaceHit> {
        self.refresh_surface_origin_cache();
        let origins = &self.surface_origin_cache;
        for (index, renderable) in self.renderable_surfaces.iter().enumerate().rev() {
            let Some(origin) = origins.get(index).copied() else {
                continue;
            };

            let root_surface_id = self.root_surface_id_for_surface(renderable.surface_id);
            if self.window_id_for_surface(root_surface_id).is_none() {
                continue;
            }
            let Some(window_id) = self.window_id_for_surface(root_surface_id) else {
                continue;
            };
            let Some(root_index) = self
                .renderable_surfaces
                .iter()
                .position(|surface| surface.surface_id == root_surface_id)
            else {
                continue;
            };
            let Some(root_origin) = origins.get(root_index).copied() else {
                continue;
            };
            let root_surface = &self.renderable_surfaces[root_index];
            let local_x = x - f64::from(root_origin.0);
            let local_y = y - f64::from(root_origin.1);
            if window_frame_action_for_local_point(
                local_x,
                local_y,
                root_surface.width,
                root_surface.height,
            )
            .is_some()
            {
                return Some(RootSurfaceHit {
                    window_id,
                    root_surface_id,
                    local_x,
                    local_y,
                    width: root_surface.width,
                    height: root_surface.height,
                });
            }

            if let Some((surface_x, surface_y)) =
                render::surface_local_point_at_origin(renderable, origin, x, y)
                && self.surface_accepts_input_at(renderable, surface_x, surface_y)
            {
                return None;
            }
        }

        None
    }

    pub(in crate::compositor) fn root_surface_id_for_surface(&self, surface_id: u32) -> u32 {
        root_surface_id_for_surface_in_placements(&self.surface_placements, surface_id)
    }

    pub(in crate::compositor) fn root_window_local_point_at(
        &mut self,
        root_surface_id: u32,
        x: f64,
        y: f64,
    ) -> Option<(f64, f64, u32, u32)> {
        self.refresh_surface_origin_cache();
        let root_index = self
            .renderable_surfaces
            .iter()
            .position(|surface| surface.surface_id == root_surface_id)?;
        let root_origin = self.surface_origin_cache.get(root_index).copied()?;
        let geometry = self.current_root_window_geometry(root_surface_id)?;
        let window_geometry = self
            .surface_window_geometries
            .get(&root_surface_id)
            .copied();
        let local_x = x
            - f64::from(root_origin.0)
            - f64::from(
                window_geometry
                    .map(|geometry| geometry.x)
                    .unwrap_or_default(),
            );
        let local_y = y
            - f64::from(root_origin.1)
            - f64::from(
                window_geometry
                    .map(|geometry| geometry.y)
                    .unwrap_or_default(),
            );
        Some((local_x, local_y, geometry.width, geometry.height))
    }

    pub(in crate::compositor) fn pointer_target_at(
        &mut self,
        x: f64,
        y: f64,
    ) -> Option<PointerTarget> {
        self.refresh_surface_origin_cache();
        let origins = &self.surface_origin_cache;
        for (index, renderable) in self.renderable_surfaces.iter().enumerate().rev() {
            let Some(origin) = origins.get(index).copied() else {
                continue;
            };
            let Some((surface_x, surface_y)) =
                render::surface_local_point_at_origin(renderable, origin, x, y)
            else {
                continue;
            };
            if !self.surface_accepts_input_at(renderable, surface_x, surface_y) {
                continue;
            }
            let Some(surface) = self.surface_resource_by_id(renderable.surface_id) else {
                continue;
            };

            return Some(PointerTarget {
                surface,
                surface_x,
                surface_y,
            });
        }

        if self.renderable_surfaces.is_empty() {
            self.focused_surface.clone().map(|surface| PointerTarget {
                surface,
                surface_x: x,
                surface_y: y,
            })
        } else {
            None
        }
    }

    pub(in crate::compositor) fn pointer_target_for_surface_at_output(
        &mut self,
        surface: &wl_surface::WlSurface,
        x: f64,
        y: f64,
    ) -> Option<PointerTarget> {
        let surface_id = compositor_surface_id(surface);
        self.refresh_surface_origin_cache();
        let index = self
            .renderable_surfaces
            .iter()
            .position(|renderable| renderable.surface_id == surface_id)?;
        let renderable = &self.renderable_surfaces[index];
        let origin = self.surface_origin_cache.get(index).copied()?;
        let (surface_x, surface_y) =
            render::surface_local_point_at_origin(renderable, origin, x, y)?;
        Some(PointerTarget {
            surface: surface.clone(),
            surface_x,
            surface_y,
        })
    }

    pub(in crate::compositor) fn surface_accepts_input_at(
        &self,
        surface: &RenderableSurface,
        surface_x: f64,
        surface_y: f64,
    ) -> bool {
        self.surface_resource_by_id(surface.surface_id)
            .and_then(|resource| {
                resource.data::<SurfaceData>().map(|data| {
                    data.input_region_contains(surface_x, surface_y, surface.width, surface.height)
                })
            })
            .unwrap_or(true)
    }

    pub(in crate::compositor) fn refresh_pointer_focus_at_last_position(&mut self) {
        if self.active_locked_pointer_binding().is_some() {
            if let Some(active) = self.active_locked_pointer_binding() {
                self.pin_locked_pointer_focus(&active);
            }
            return;
        }

        let Some(target) = self.pointer_target_at(self.last_pointer_x, self.last_pointer_y) else {
            self.clear_pointer_focus();
            pointer_debug_log("post-unlock focus target=none");
            return;
        };

        pointer_debug_log(format!(
            "post-unlock focus target={} x={} y={}",
            compositor_surface_id(&target.surface),
            target.surface_x,
            target.surface_y
        ));
        self.ensure_pointer_focus(&target.surface);
        self.send_pointer_enter_if_needed(&target);
    }

    pub(in crate::compositor) fn refresh_pointer_focus_after_implicit_grab(
        &mut self,
        old_surface_id: Option<u32>,
    ) {
        if self.active_locked_pointer_binding().is_some() {
            self.refresh_pointer_focus_at_last_position();
            return;
        }

        let target = self.pointer_target_at(self.last_pointer_x, self.last_pointer_y);
        let new_surface_id = target
            .as_ref()
            .map(|target| compositor_surface_id(&target.surface));
        pointer_debug_log(format!(
            "post-grab focus surface={} -> {}",
            old_surface_id
                .map(|surface_id| surface_id.to_string())
                .unwrap_or_else(|| "none".to_string()),
            new_surface_id
                .map(|surface_id| surface_id.to_string())
                .unwrap_or_else(|| "none".to_string())
        ));
        let Some(target) = target else {
            self.clear_pointer_focus();
            return;
        };
        if !self.pointer_target_allowed_by_popup_grab(&target) {
            self.clear_pointer_focus();
            return;
        }
        self.ensure_pointer_focus(&target.surface);
        self.send_pointer_enter_if_needed(&target);
    }

    pub(in crate::compositor) fn restore_locked_pointer_position(
        &mut self,
        surface: &wl_surface::WlSurface,
        cursor_position_hint: Option<(f64, f64)>,
    ) -> Option<OutputPosition> {
        if let Some((surface_x, surface_y)) = cursor_position_hint {
            if !surface_x.is_finite() || !surface_y.is_finite() {
                pointer_debug_log(format!(
                    "pointer.unlock restore_source=committed_hint ignored reason=non_finite hint=({},{})",
                    surface_x, surface_y
                ));
            } else if let Some((output_x, output_y)) =
                self.output_position_for_valid_cursor_hint(surface, surface_x, surface_y)
            {
                self.last_pointer_x = output_x;
                self.last_pointer_y = output_y;
                pointer_debug_log(format!(
                    "pointer.unlock restore_source=committed_hint hint=({surface_x},{surface_y}) restore_output=({output_x},{output_y})"
                ));
                return Some(OutputPosition {
                    x: output_x,
                    y: output_y,
                });
            } else {
                pointer_debug_log(format!(
                    "pointer.unlock restore_source=committed_hint ignored reason=unresolved hint=({surface_x},{surface_y})"
                ));
            }
        }

        let fallback_position = self
            .active_locked_pointer_routing
            .as_ref()
            .filter(|active| same_surface_resource(&active.surface, surface))
            .map(|active| active.activation_anchor);
        let Some(position) = fallback_position else {
            pointer_debug_log("pointer.unlock restore_source=none restore_output=unchanged");
            return None;
        };
        self.last_pointer_x = position.x;
        self.last_pointer_y = position.y;
        pointer_debug_log(format!(
            "pointer.unlock restore_source=activation_anchor restore_output=({},{})",
            position.x, position.y
        ));
        Some(position)
    }

    pub(in crate::compositor) fn output_position_for_valid_cursor_hint(
        &mut self,
        surface: &wl_surface::WlSurface,
        surface_x: f64,
        surface_y: f64,
    ) -> Option<(f64, f64)> {
        let surface_id = compositor_surface_id(surface);
        self.refresh_surface_origin_cache();
        let index = self
            .renderable_surfaces
            .iter()
            .position(|renderable| renderable.surface_id == surface_id)?;
        let renderable = &self.renderable_surfaces[index];
        if surface_x < 0.0
            || surface_y < 0.0
            || surface_x >= f64::from(renderable.width)
            || surface_y >= f64::from(renderable.height)
        {
            pointer_debug_log(format!(
                "pointer.unlock restore_source=committed_hint ignored reason=out_of_bounds hint=({},{}) size={}x{}",
                surface_x, surface_y, renderable.width, renderable.height
            ));
            return None;
        }
        let origin = self.surface_origin_cache.get(index).copied()?;
        Some((
            f64::from(origin.0) + surface_x,
            f64::from(origin.1) + surface_y,
        ))
    }

    pub(in crate::compositor) fn surface_resource_by_id(
        &self,
        surface_id: u32,
    ) -> Option<wl_surface::WlSurface> {
        self.surface_resources.get(&surface_id).cloned()
    }

    pub(in crate::compositor) fn ensure_pointer_focus(&mut self, surface: &wl_surface::WlSurface) {
        if let Some(active) = self.active_locked_pointer_binding()
            && !same_surface_resource(&active.surface, surface)
        {
            pointer_debug_log(format!(
                "pointer focus change suppressed by locked route id={} locked_surface={} requested={}",
                active.constraint_id,
                compositor_surface_id(&active.surface),
                compositor_surface_id(surface)
            ));
            self.pin_locked_pointer_focus(&active);
            return;
        }
        if let Some(active) = self.active_confined_pointer_binding()
            && !same_surface_resource(&active.surface, surface)
        {
            self.pin_confined_pointer_focus(&active);
            return;
        }
        if self
            .pointer_surface
            .as_ref()
            .is_some_and(|current| same_surface_resource(current, surface))
        {
            return;
        }

        if self.pointer_surface.is_some() {
            self.clear_pointer_focus();
        }
        self.pointer_surface = Some(surface.clone());
    }

    pub(in crate::compositor) fn pointer_resource_entered_surface(
        &self,
        pointer: &wl_pointer::WlPointer,
        surface: &wl_surface::WlSurface,
    ) -> bool {
        self.pointer_entered_surfaces
            .iter()
            .any(|(resource, entered_surface)| {
                same_wayland_resource(resource, pointer)
                    && same_surface_resource(entered_surface, surface)
            })
    }

    pub(in crate::compositor) fn pointer_has_current_enter_serial(
        &self,
        pointer: &wl_pointer::WlPointer,
        serial: u32,
        surface: &wl_surface::WlSurface,
    ) -> bool {
        self.pointer_enter_serials.iter().any(|entry| {
            same_wayland_resource(&entry.pointer, pointer)
                && same_surface_resource(&entry.surface, surface)
                && entry.serial == serial
        })
    }

    pub(in crate::compositor) fn pointer_has_current_enter_serial_for_client(
        &self,
        pointer: &wl_pointer::WlPointer,
        serial: u32,
        surface: &wl_surface::WlSurface,
    ) -> bool {
        resource_belongs_to_surface_client(pointer, surface)
            && self.validate_set_cursor_serial(serial, surface)
    }

    pub(in crate::compositor) fn warp_pointer_protocol_request(
        &mut self,
        surface: wl_surface::WlSurface,
        pointer: wl_pointer::WlPointer,
        surface_x: f64,
        surface_y: f64,
        serial: u32,
    ) {
        let reject = |reason: &str| {
            pointer_debug_log(format!(
                "pointer_warp rejected pointer={} surface={} serial={} local=({},{}) reason={}",
                pointer.id().protocol_id(),
                compositor_surface_id(&surface),
                serial,
                surface_x,
                surface_y,
                reason
            ));
        };
        if !pointer.is_alive() || !surface.is_alive() {
            reject("dead_resource");
            return;
        }
        if !surface_x.is_finite() || !surface_y.is_finite() {
            reject("non_finite");
            return;
        }
        if !resource_belongs_to_surface_client(&pointer, &surface) {
            reject("wrong_client_pointer");
            return;
        }
        if !self
            .pointer_resources
            .iter()
            .any(|resource| same_wayland_resource(resource, &pointer))
        {
            reject("unknown_pointer");
            return;
        }
        let focused_surface = self
            .implicit_pointer_grab
            .as_ref()
            .map(|grab| grab.surface.clone())
            .or_else(|| self.pointer_surface.clone());
        let Some(focused_surface) = focused_surface else {
            reject("no_pointer_focus");
            return;
        };
        if !same_surface_resource(&focused_surface, &surface) {
            reject("surface_not_focused");
            return;
        }
        if !self.pointer_has_current_enter_serial_for_client(&pointer, serial, &surface) {
            reject("invalid_serial");
            return;
        }
        let Some(position) =
            self.valid_cursor_hint_output_position(&surface, Some((surface_x, surface_y)))
        else {
            reject("out_of_surface");
            return;
        };
        pointer_debug_log(format!(
            "pointer_warp accepted pointer={} serial={} local=({},{}) output=({},{}) matches_pending_unlock={}",
            pointer.id().protocol_id(),
            serial,
            surface_x,
            surface_y,
            position.x,
            position.y,
            self.pending_locked_pointer_reveal_matches(&pointer, &surface)
        ));
        let matches_pending_unlock = self.pending_locked_pointer_reveal_matches(&pointer, &surface);
        self.apply_pointer_warp(position, true);
        if matches_pending_unlock {
            if let Some(pending) = self.pending_locked_pointer_reveal.as_mut() {
                pending.fallback_position = Some(position);
            }
            self.finalize_pending_locked_pointer_reveal("matching_client_warp");
        }
    }

    pub(in crate::compositor) fn remember_pointer_enter_serial(
        &mut self,
        pointer: &wl_pointer::WlPointer,
        surface: &wl_surface::WlSurface,
        serial: u32,
    ) {
        self.pointer_enter_serials
            .retain(|entry| !same_wayland_resource(&entry.pointer, pointer));
        self.pointer_enter_serials.push(PointerEnterSerial {
            pointer: pointer.clone(),
            surface: surface.clone(),
            serial,
        });
    }

    pub(in crate::compositor) fn forget_pointer_enter_serial(
        &mut self,
        pointer: &wl_pointer::WlPointer,
    ) {
        self.pointer_enter_serials
            .retain(|entry| !same_wayland_resource(&entry.pointer, pointer));
    }

    pub(in crate::compositor) fn synchronize_pointer_resource_focus(
        &mut self,
        pointer: &wl_pointer::WlPointer,
    ) -> bool {
        let Some(focused_surface) = self.pointer_surface.clone() else {
            return false;
        };
        if !pointer.is_alive() || !resource_belongs_to_surface_client(pointer, &focused_surface) {
            return false;
        }
        if self.pointer_resource_entered_surface(pointer, &focused_surface) {
            return true;
        }
        let Some(target) = self.pointer_target_at(self.last_pointer_x, self.last_pointer_y) else {
            return false;
        };
        if !same_surface_resource(&target.surface, &focused_surface) {
            return false;
        }
        self.send_pointer_enter_to_resource(pointer, &target);
        true
    }

    pub(in crate::compositor) fn send_pointer_enter_to_resource(
        &mut self,
        pointer: &wl_pointer::WlPointer,
        target: &PointerTarget,
    ) {
        if let Some(index) = self
            .pointer_entered_surfaces
            .iter()
            .position(|(resource, _)| same_wayland_resource(resource, pointer))
        {
            if same_surface_resource(&self.pointer_entered_surfaces[index].1, &target.surface) {
                return;
            }

            let (_, previous_surface) = self.pointer_entered_surfaces.remove(index);
            self.forget_pointer_enter_serial(pointer);
            if resource_belongs_to_surface_client(pointer, &previous_surface) {
                let serial = self.next_configure_serial();
                let _ = pointer.send_event(wl_pointer::Event::Leave {
                    serial,
                    surface: previous_surface,
                });
                send_pointer_frame_if_supported(pointer);
            }
        }

        let serial = self.next_configure_serial();
        let _ = pointer.send_event(wl_pointer::Event::Enter {
            serial,
            surface: target.surface.clone(),
            surface_x: target.surface_x,
            surface_y: target.surface_y,
        });
        pointer_debug_log(format!(
            "wl_pointer {} synchronized enter for surface {}",
            pointer.id().protocol_id(),
            compositor_surface_id(&target.surface)
        ));
        self.remember_input_serial(
            serial,
            target.surface.clone(),
            InputSerialKind::PointerEnter,
        );
        self.remember_pointer_enter_serial(pointer, &target.surface, serial);
        send_pointer_frame_if_supported(pointer);
        self.pointer_entered_surfaces
            .push((pointer.clone(), target.surface.clone()));
    }

    pub(in crate::compositor) fn send_pointer_enter_if_needed(&mut self, target: &PointerTarget) {
        self.pointer_resources.retain(Resource::is_alive);
        let pointers = self
            .pointer_resources
            .iter()
            .filter(|pointer| resource_belongs_to_surface_client(*pointer, &target.surface))
            .cloned()
            .collect::<Vec<_>>();

        for pointer in pointers {
            self.send_pointer_enter_to_resource(&pointer, target);
        }
        let surface_id = compositor_surface_id(&target.surface);
        let constraint_ids = self
            .pointer_constraints
            .values()
            .filter(|constraint| compositor_surface_id(&constraint.surface) == surface_id)
            .map(|constraint| constraint.id)
            .collect::<Vec<_>>();
        for constraint_id in constraint_ids {
            self.maybe_request_pointer_constraint_activation(constraint_id);
        }
    }

    pub(in crate::compositor) fn clear_pointer_focus(&mut self) {
        if self.window_interaction.is_some() {
            self.clear_window_interaction_state(WindowInteractionEndReason::FocusLoss);
        }
        if let Some(active) = self.active_locked_pointer_binding() {
            pointer_debug_log(format!(
                "pointer focus clear suppressed by locked route id={} surface={}",
                active.constraint_id,
                compositor_surface_id(&active.surface)
            ));
            self.pin_locked_pointer_focus(&active);
            return;
        }
        if let Some(active) = self.active_confined_pointer_binding() {
            pointer_debug_log(format!(
                "pointer focus clear suppressed by confined route id={} surface={}",
                active.constraint_id,
                compositor_surface_id(&active.surface)
            ));
            self.pin_confined_pointer_focus(&active);
            return;
        }
        if let Some(surface_id) = self.pointer_surface.as_ref().map(compositor_surface_id) {
            pointer_debug_log(format!(
                "pointer focus loss deactivating constraints surface={}",
                surface_id
            ));
            self.deactivate_pointer_constraints_for_surface_focus_loss(surface_id, true);
        }
        let cleared_client_cursor = self.active_client_cursor.take().is_some();
        self.cursor_visibility.client_hidden_pointer = None;
        self.cursor_visibility.client_cursor_pointer = None;
        if cleared_client_cursor {
            self.advance_render_generation(RenderGenerationCause::CursorState);
            pointer_debug_log("cursor cleanup reason=pointer-focus-loss");
        }
        self.sync_cursor_visibility_request();
        self.pointer_surface = None;
        self.pointer_resources.retain(Resource::is_alive);
        let pointers = self.pointer_resources.clone();
        for pointer in pointers {
            let Some(index) = self
                .pointer_entered_surfaces
                .iter()
                .position(|(resource, _)| same_wayland_resource(resource, &pointer))
            else {
                continue;
            };
            let (_, surface) = self.pointer_entered_surfaces.remove(index);
            self.forget_pointer_enter_serial(&pointer);
            if !resource_belongs_to_surface_client(&pointer, &surface) {
                continue;
            }
            let serial = self.next_configure_serial();
            let _ = pointer.send_event(wl_pointer::Event::Leave { serial, surface });
            send_pointer_frame_if_supported(&pointer);
        }
    }
}
