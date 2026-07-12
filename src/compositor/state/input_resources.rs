use super::*;

impl CompositorState {
    pub(in crate::compositor) fn clear_pointer_button_state_for_removed_surfaces(
        &mut self,
        removed_surface_ids: &[u32],
        reason: &'static str,
    ) {
        self.cancel_implicit_pointer_grab_for_surface_ids(removed_surface_ids, reason);
        self.held_pointer_buttons.retain(|press| {
            !removed_surface_ids.contains(&compositor_surface_id(&press.surface))
                && !removed_surface_ids.contains(&press.root_surface_id)
        });
        if self.last_pointer_press.as_ref().is_some_and(|press| {
            removed_surface_ids.contains(&compositor_surface_id(&press.surface))
                || removed_surface_ids.contains(&press.root_surface_id)
        }) {
            self.last_pointer_press = None;
        }
    }

    pub(in crate::compositor) fn register_keyboard(&mut self, keyboard: wl_keyboard::WlKeyboard) {
        if self
            .keyboard_resources
            .iter()
            .any(|resource| same_wayland_resource(resource, &keyboard))
        {
            return;
        }
        self.keyboard_resources.push(keyboard);
        if let Some(surface) = self.focused_surface.clone() {
            self.ensure_keyboard_focus(&surface);
        }
    }

    pub(in crate::compositor) fn register_pointer(&mut self, pointer: wl_pointer::WlPointer) {
        if self
            .pointer_resources
            .iter()
            .any(|resource| same_wayland_resource(resource, &pointer))
        {
            return;
        }
        self.pointer_resources.push(pointer.clone());
        self.synchronize_pointer_resource_focus(&pointer);
    }

    pub(in crate::compositor) fn unregister_keyboard(
        &mut self,
        keyboard: &wl_keyboard::WlKeyboard,
    ) {
        self.keyboard_resources
            .retain(|resource| !same_wayland_resource(resource, keyboard));
    }

    pub(in crate::compositor) fn unregister_pointer(&mut self, pointer: &wl_pointer::WlPointer) {
        self.pointer_resources
            .retain(|resource| !same_wayland_resource(resource, pointer));
        self.pointer_entered_surfaces
            .retain(|(resource, _)| !same_wayland_resource(resource, pointer));
        self.pointer_enter_serials
            .retain(|entry| !same_wayland_resource(&entry.pointer, pointer));
        self.relative_pointer_resources
            .retain(|resource| !same_wayland_resource(&resource.source_pointer, pointer));
        self.deactivate_pointer_constraints_for_pointer(pointer, false);
        if self
            .active_client_cursor
            .as_ref()
            .is_some_and(|active| same_wayland_resource(&active.pointer, pointer))
        {
            self.active_client_cursor = None;
            self.advance_render_generation(RenderGenerationCause::CursorState);
            pointer_debug_log(format!(
                "cursor cleanup pointer={} reason=owning-pointer-destroyed",
                pointer.id().protocol_id()
            ));
        }
        if self
            .cursor_visibility
            .client_hidden_pointer
            .as_ref()
            .is_some_and(|hidden_pointer| same_wayland_resource(hidden_pointer, pointer))
        {
            self.cursor_visibility.client_hidden_pointer = None;
            self.sync_cursor_visibility_request();
        }
        if self
            .cursor_visibility
            .client_cursor_pointer
            .as_ref()
            .is_some_and(|cursor_pointer| same_wayland_resource(cursor_pointer, pointer))
        {
            self.cursor_visibility.client_cursor_pointer = None;
            self.sync_cursor_visibility_request();
        }
    }

    pub(in crate::compositor) fn set_pointer_cursor(
        &mut self,
        pointer: &wl_pointer::WlPointer,
        serial: u32,
        surface: Option<wl_surface::WlSurface>,
        hotspot_x: i32,
        hotspot_y: i32,
    ) {
        let Some(pointer_surface) = self.pointer_surface.as_ref() else {
            return;
        };
        let focused_client = resource_belongs_to_surface_client(pointer, pointer_surface);
        let exact_serial = self.pointer_has_current_enter_serial(pointer, serial, pointer_surface);
        let valid = focused_client && exact_serial;
        pointer_debug_log(format!(
            "cursor request pointer={} client={} serial={} valid={} exact_serial={} focused_client={} null={}",
            pointer.id().protocol_id(),
            wayland_resource_client_label(pointer),
            serial,
            valid,
            exact_serial,
            focused_client,
            surface.is_none()
        ));
        if !valid {
            pointer_debug_log("cursor request ignored reason=invalid-focus-or-enter-serial");
            return;
        }
        let resolves_pending_unlock = self
            .pending_locked_pointer_reveal
            .as_ref()
            .is_some_and(|pending| same_wayland_resource(&pending.pointer, pointer));
        let Some(surface) = surface else {
            let changed = self.active_client_cursor.take().is_some();
            self.cursor_visibility.client_hidden_pointer = Some(pointer.clone());
            self.cursor_visibility.client_cursor_pointer = None;
            if changed {
                self.advance_render_generation(RenderGenerationCause::CursorState);
            }
            self.sync_cursor_visibility_request();
            if resolves_pending_unlock {
                self.finalize_pending_locked_pointer_reveal("client_hidden_cursor");
            }
            return;
        };
        let surface_id = compositor_surface_id(&surface);
        if let Err(error) = self.assign_surface_role(surface_id, SurfaceRole::Cursor) {
            pointer_debug_log(format!(
                "cursor request rejected pointer={} surface={} reason={}",
                pointer.id().protocol_id(),
                surface_id,
                error.message()
            ));
            return;
        }
        self.cursor_surface_ids.insert(surface_id);
        self.unmap_surface_content(surface_id);
        let changed = self.active_client_cursor.as_ref().is_none_or(|active| {
            !same_wayland_resource(&active.pointer, pointer)
                || active.surface_id != surface_id
                || active.hotspot_x != hotspot_x
                || active.hotspot_y != hotspot_y
        });
        self.active_client_cursor = Some(ActiveClientCursor {
            pointer: pointer.clone(),
            surface_id,
            hotspot_x,
            hotspot_y,
        });
        self.cursor_visibility.client_hidden_pointer = None;
        self.cursor_visibility.client_cursor_pointer = Some(pointer.clone());
        pointer_debug_log(format!(
            "cursor request client_surface pointer={} surface={} hotspot=({}, {})",
            pointer.id().protocol_id(),
            surface_id,
            hotspot_x,
            hotspot_y
        ));
        if changed {
            self.advance_render_generation(RenderGenerationCause::CursorState);
        }
        self.sync_cursor_visibility_request();
        if resolves_pending_unlock {
            self.finalize_pending_locked_pointer_reveal("client_cursor_surface");
        }
    }

    pub(in crate::compositor) fn is_cursor_surface(&self, surface_id: u32) -> bool {
        self.cursor_surface_ids.contains(&surface_id)
    }

    pub(in crate::compositor) fn client_cursor_render_state(
        &self,
    ) -> Option<ClientCursorRenderState<'_>> {
        if self.interaction_cursor_override.is_some() {
            return None;
        }
        if self.cursor_visibility.lock_hidden_constraint_id.is_some() {
            return None;
        }
        let active = self.active_client_cursor.as_ref()?;
        let surface = self.client_cursor_surfaces.get(&active.surface_id)?;
        Some(ClientCursorRenderState {
            surface,
            logical_x: (self.last_pointer_x.round() as i32).saturating_sub(active.hotspot_x),
            logical_y: (self.last_pointer_y.round() as i32).saturating_sub(active.hotspot_y),
        })
    }

    pub(in crate::compositor) fn active_client_cursor_has_content(&self) -> bool {
        self.active_client_cursor
            .as_ref()
            .is_some_and(|active| self.client_cursor_surfaces.contains_key(&active.surface_id))
    }

    pub(in crate::compositor) fn send_keyboard_key(&mut self, key: u32, pressed: bool) {
        let modifiers_changed = self.keyboard_modifiers.update_key(key, pressed);
        let Some(surface) = self.focused_surface.clone() else {
            return;
        };
        let state = if pressed {
            wl_keyboard::KeyState::Pressed
        } else {
            wl_keyboard::KeyState::Released
        };
        let time = wayland_event_time();

        self.ensure_keyboard_focus(&surface);

        let serial = self.next_configure_serial();
        self.remember_input_serial(serial, surface.clone());
        for keyboard in self
            .keyboard_resources
            .iter()
            .filter(|keyboard| resource_belongs_to_surface_client(*keyboard, &surface))
        {
            let _ = keyboard.send_event(wl_keyboard::Event::Key {
                serial,
                time,
                key,
                state: WEnum::Value(state),
            });
        }
        if modifiers_changed {
            self.send_keyboard_modifiers(&surface, serial);
        }
    }

    pub(in crate::compositor) fn ensure_keyboard_focus(&mut self, surface: &wl_surface::WlSurface) {
        if self
            .keyboard_surface
            .as_ref()
            .is_some_and(|current| same_surface_resource(current, surface))
        {
            return;
        }

        self.clear_keyboard_focus();
        self.keyboard_resources.retain(Resource::is_alive);
        let keyboards = self
            .keyboard_resources
            .iter()
            .filter(|keyboard| resource_belongs_to_surface_client(*keyboard, surface))
            .cloned()
            .collect::<Vec<_>>();
        if keyboards.is_empty() {
            return;
        }

        let serial = self.next_configure_serial();
        for keyboard in keyboards {
            let _ = keyboard.send_event(wl_keyboard::Event::Enter {
                serial,
                surface: surface.clone(),
                keys: Vec::new(),
            });
            let _ = keyboard.send_event(wl_keyboard::Event::Modifiers {
                serial,
                mods_depressed: self.keyboard_modifiers.mods_depressed(),
                mods_latched: 0,
                mods_locked: self.keyboard_modifiers.mods_locked(),
                group: 0,
            });
        }
        pointer_debug_log(format!(
            "keyboard enter surface={} client={}",
            compositor_surface_id(surface),
            wayland_resource_client_label(surface)
        ));
        self.keyboard_surface = Some(surface.clone());
        self.publish_clipboard_to_focused_client();
    }

    pub(in crate::compositor) fn send_keyboard_modifiers(
        &mut self,
        surface: &wl_surface::WlSurface,
        serial: u32,
    ) {
        self.keyboard_resources.retain(Resource::is_alive);
        for keyboard in self
            .keyboard_resources
            .iter()
            .filter(|keyboard| resource_belongs_to_surface_client(*keyboard, surface))
        {
            let _ = keyboard.send_event(wl_keyboard::Event::Modifiers {
                serial,
                mods_depressed: self.keyboard_modifiers.mods_depressed(),
                mods_latched: 0,
                mods_locked: self.keyboard_modifiers.mods_locked(),
                group: 0,
            });
        }
    }

    pub(in crate::compositor) fn clear_keyboard_focus(&mut self) {
        let Some(surface) = self.keyboard_surface.take() else {
            return;
        };
        self.keyboard_resources.retain(Resource::is_alive);
        let keyboards = self
            .keyboard_resources
            .iter()
            .filter(|keyboard| resource_belongs_to_surface_client(*keyboard, &surface))
            .cloned()
            .collect::<Vec<_>>();
        if keyboards.is_empty() {
            return;
        }

        let serial = self.next_configure_serial();
        for keyboard in keyboards {
            let _ = keyboard.send_event(wl_keyboard::Event::Leave {
                serial,
                surface: surface.clone(),
            });
        }
        pointer_debug_log(format!(
            "keyboard leave surface={} client={}",
            compositor_surface_id(&surface),
            wayland_resource_client_label(&surface)
        ));
    }

    pub(in crate::compositor) fn send_pointer_motion(&mut self, x: f64, y: f64) {
        if let Some(active) = self.active_locked_pointer_binding() {
            pointer_debug_log(format!(
                "pointer.motion locked=true absolute_suppressed=true requested_output=({},{}) anchor_output=({},{})",
                x, y, active.activation_anchor.x, active.activation_anchor.y
            ));
            self.pin_locked_pointer_focus(&active);
            return;
        }
        if self.active_confined_pointer_binding().is_some() {
            self.send_confined_pointer_motion(x, y);
            return;
        }
        self.update_pointer_position(x, y);
        if self.send_implicit_pointer_grab_motion(x, y) {
            return;
        }
        let Some(target) = self.pointer_target_at(x, y) else {
            self.clear_pointer_focus();
            return;
        };
        if !self.pointer_target_allowed_by_popup_grab(&target) {
            self.clear_pointer_focus();
            return;
        }
        let time = wayland_event_time();
        self.ensure_pointer_focus(&target.surface);
        self.send_pointer_enter_if_needed(&target);

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

    pub(in crate::compositor) fn update_pointer_position(&mut self, x: f64, y: f64) {
        let changed = self.last_pointer_x != x || self.last_pointer_y != y;
        let moves_visible_cursor = changed
            && (self.interaction_cursor_override.is_some()
                || self.client_cursor_render_state().is_some());
        self.last_pointer_x = x;
        self.last_pointer_y = y;
        if moves_visible_cursor {
            self.advance_render_generation(RenderGenerationCause::CursorMotion);
        }
    }

    pub(in crate::compositor) fn update_pointer_position_without_client_dispatch(
        &mut self,
        x: f64,
        y: f64,
    ) -> bool {
        let before = self.render_generation;
        self.update_pointer_position(x, y);
        self.render_generation != before
    }

    pub(in crate::compositor) fn send_pointer_motion_sample(
        &mut self,
        sample: PointerMotionSample,
    ) {
        self.last_pointer_motion_usec = Some(sample.timestamp_usec);
        if let Some(relative) = sample.relative {
            self.last_relative_pointer_motion = Some(relative);
            self.send_relative_pointer_motion(sample.timestamp_usec, relative);
        }
        if let Some(position) = sample.absolute {
            let locked_surface_id = self
                .pointer_surface
                .as_ref()
                .map(compositor_surface_id)
                .filter(|surface_id| self.pointer_constraint.filters_absolute_motion(*surface_id));
            if locked_surface_id.is_none() {
                self.send_pointer_motion(position.x, position.y);
            } else if let Some(surface_id) = locked_surface_id {
                pointer_debug_log(format!(
                    "pointer.motion locked=true absolute_suppressed=true output=({},{}) surface={}",
                    position.x, position.y, surface_id
                ));
            }
        }
    }
}
