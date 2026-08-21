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
        if let Some(surface) = self.keyboard_surface.clone() {
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
        let owned_active_cursor = self
            .focused_client_cursor
            .as_ref()
            .is_some_and(|choice| same_wayland_resource(choice.pointer(), pointer));
        let owned_cursor_visibility = self
            .cursor_visibility
            .client_hidden_pointer
            .as_ref()
            .is_some_and(|owner| same_wayland_resource(owner, pointer))
            || self
                .cursor_visibility
                .client_cursor_pointer
                .as_ref()
                .is_some_and(|owner| same_wayland_resource(owner, pointer));
        let preserve_client_cursor_claim = owned_active_cursor || owned_cursor_visibility;
        if owned_active_cursor {
            self.focused_client_cursor = None;
            self.advance_render_generation(RenderGenerationCause::CursorState);
        }
        if preserve_client_cursor_claim {
            // wl_pointer.release does not reset the cursor selected for the
            // current focus. Keep the client claim hidden until focus changes
            // or another live pointer supplies a replacement cursor.
            self.cursor_visibility.client_hidden_pointer = Some(pointer.clone());
            self.cursor_visibility.client_cursor_pointer = None;
            self.focused_client_cursor = Some(ClientCursorChoice::Hidden {
                pointer: pointer.clone(),
            });
        }
        self.pointer_resources
            .retain(|resource| !same_wayland_resource(resource, pointer));
        self.pointer_entered_surfaces
            .retain(|(resource, _)| !same_wayland_resource(resource, pointer));
        self.pointer_enter_serials
            .retain(|entry| !same_wayland_resource(&entry.pointer, pointer));
        self.relative_pointer_resources
            .retain(|resource| !same_wayland_resource(&resource.source_pointer, pointer));
        self.deactivate_pointer_constraints_for_pointer(pointer, false);
        if owned_active_cursor {
            pointer_debug_log_lazy(|| {
                format!(
                    "cursor cleanup pointer={} reason=owning-pointer-destroyed",
                    pointer.id().protocol_id()
                )
            });
        }
        if preserve_client_cursor_claim {
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
        pointer_debug_log_lazy(|| {
            format!(
                "cursor request pointer={} client={} serial={} valid={} exact_serial={} focused_client={} null={}",
                pointer.id().protocol_id(),
                wayland_resource_client_label(pointer),
                serial,
                valid,
                exact_serial,
                focused_client,
                surface.is_none()
            )
        });
        if !valid {
            pointer_debug_log("cursor request ignored reason=invalid-focus-or-enter-serial");
            return;
        }
        let resolves_pending_unlock = self
            .pending_locked_pointer_reveal
            .as_ref()
            .is_some_and(|pending| same_wayland_resource(&pending.pointer, pointer));
        let Some(surface) = surface else {
            let choice = ClientCursorChoice::Hidden {
                pointer: pointer.clone(),
            };
            let changed = self
                .focused_client_cursor
                .as_ref()
                .is_none_or(|current| !current.is_same_as(&choice));
            self.focused_client_cursor = Some(choice);
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
            pointer_debug_log_lazy(|| {
                format!(
                    "cursor request rejected pointer={} surface={} reason={}",
                    pointer.id().protocol_id(),
                    surface_id,
                    error.message()
                )
            });
            return;
        }
        self.cursor_surface_ids.insert(surface_id);
        self.unmap_surface_content(surface_id);
        let choice = ClientCursorChoice::Surface(ActiveClientCursor {
            pointer: pointer.clone(),
            surface_id,
            hotspot_x,
            hotspot_y,
        });
        let changed = self
            .focused_client_cursor
            .as_ref()
            .is_none_or(|current| !current.is_same_as(&choice));
        self.focused_client_cursor = Some(choice);
        self.cursor_visibility.client_hidden_pointer = None;
        self.cursor_visibility.client_cursor_pointer = Some(pointer.clone());
        pointer_debug_log_lazy(|| {
            format!(
                "cursor request client_surface pointer={} surface={} hotspot=({}, {})",
                pointer.id().protocol_id(),
                surface_id,
                hotspot_x,
                hotspot_y
            )
        });
        if changed {
            self.advance_render_generation(RenderGenerationCause::CursorState);
        }
        self.sync_cursor_visibility_request();
        if resolves_pending_unlock {
            self.finalize_pending_locked_pointer_reveal("client_cursor_surface");
        }
    }

    pub(in crate::compositor) fn set_pointer_shape(
        &mut self,
        pointer: &wl_pointer::WlPointer,
        serial: u32,
        shape: u32,
    ) {
        let Some(pointer_surface) = self.pointer_surface.as_ref() else {
            return;
        };
        let focused_client = resource_belongs_to_surface_client(pointer, pointer_surface);
        let exact_serial = self.pointer_has_current_enter_serial(pointer, serial, pointer_surface);
        if !focused_client || !exact_serial {
            pointer_debug_log("shape request ignored reason=invalid-focus-or-enter-serial");
            return;
        }
        let resolves_pending_unlock = self
            .pending_locked_pointer_reveal
            .as_ref()
            .is_some_and(|pending| same_wayland_resource(&pending.pointer, pointer));
        let choice = ClientCursorChoice::Shape {
            pointer: pointer.clone(),
            shape,
        };
        let changed = self
            .focused_client_cursor
            .as_ref()
            .is_none_or(|current| !current.is_same_as(&choice));
        self.focused_client_cursor = Some(choice);
        self.cursor_visibility.client_hidden_pointer = None;
        self.cursor_visibility.client_cursor_pointer = Some(pointer.clone());
        pointer_debug_log_lazy(|| {
            format!(
                "cursor request client_shape pointer={} shape={} serial={}",
                pointer.id().protocol_id(),
                shape,
                serial
            )
        });
        if changed {
            self.advance_render_generation(RenderGenerationCause::CursorState);
        }
        self.sync_cursor_visibility_request();
        if resolves_pending_unlock {
            self.finalize_pending_locked_pointer_reveal("client_shape_cursor");
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
        if self.cursor_visibility.client_hidden_pointer.is_some() {
            return None;
        }
        let active = self.focused_client_cursor.as_ref()?.surface()?;
        let surface = self.client_cursor_surfaces.get(&active.surface_id)?;
        Some(ClientCursorRenderState {
            surface,
            logical_x: (self.last_pointer_x.round() as i32).saturating_sub(active.hotspot_x),
            logical_y: (self.last_pointer_y.round() as i32).saturating_sub(active.hotspot_y),
            hotspot_x: active.hotspot_x,
            hotspot_y: active.hotspot_y,
        })
    }

    pub(in crate::compositor) fn active_client_cursor_has_content(&self) -> bool {
        self.focused_client_cursor
            .as_ref()
            .and_then(ClientCursorChoice::surface)
            .is_some_and(|active| self.client_cursor_surfaces.contains_key(&active.surface_id))
    }

    pub(in crate::compositor) fn client_cursor_explicitly_hidden(&self) -> bool {
        self.cursor_visibility.client_hidden_pointer.is_some()
            || self
                .focused_client_cursor
                .as_ref()
                .is_some_and(ClientCursorChoice::is_hidden)
    }

    pub(in crate::compositor) fn client_cursor_shape(&self) -> Option<u32> {
        match self.focused_client_cursor.as_ref()? {
            ClientCursorChoice::Shape { shape, .. } => Some(*shape),
            ClientCursorChoice::Hidden { .. } | ClientCursorChoice::Surface(_) => None,
        }
    }

    pub(in crate::compositor) fn send_keyboard_key(&mut self, key: u32, pressed: bool) {
        if pressed {
            self.pressed_keys.insert(key);
        } else {
            self.pressed_keys.remove(&key);
        }
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
        self.remember_input_serial(
            serial,
            surface.clone(),
            InputSerialKind::KeyboardKeyPress { key },
        );
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
        if self.pointer_hit_instrumentation_enabled {
            self.pointer_hit_metrics.keyboard_focus_reconciliations += 1;
        }
        if self
            .keyboard_surface
            .as_ref()
            .is_some_and(|current| same_surface_resource(current, surface))
        {
            return;
        }

        let previous_client_id = self.keyboard_focused_client_id();
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

        let target_client_id = surface.client().map(|client| client.id());
        if previous_client_id != target_client_id {
            if let Some(previous_client_id) = previous_client_id.as_ref() {
                self.publish_primary_clear_to_client(previous_client_id);
            }
            if let Some(target_client_id) = target_client_id.as_ref() {
                self.publish_clipboard_to_client(target_client_id);
                if self
                    .selection_state
                    .active_selection(SelectionKind::Primary)
                    .is_some()
                {
                    self.publish_primary_to_client(target_client_id);
                }
            }
        }

        let serial = self.next_configure_serial();
        for keyboard in keyboards {
            let _ = keyboard.send_event(wl_keyboard::Event::Enter {
                serial,
                surface: surface.clone(),
                keys: self
                    .pressed_keys
                    .iter()
                    .flat_map(|key| key.to_ne_bytes())
                    .collect(),
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
        self.refresh_keyboard_shortcut_inhibition();
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
            self.refresh_keyboard_shortcut_inhibition();
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
        self.refresh_keyboard_shortcut_inhibition();
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
        if self.window_interaction_active() {
            self.update_window_interaction(x, y);
            self.update_pointer_position(x, y);
            let _ = self.send_window_interaction_pointer_motion(
                u64::from(wayland_event_time()).saturating_mul(1_000),
                x,
                y,
            );
            return;
        }
        self.update_pointer_position_state(x, y);
        let hit = self.pointer_scene_hit_at(x, y);
        self.update_drag_target_at(x, y);
        if self.send_implicit_pointer_grab_motion(x, y) {
            self.update_decoration_hover_for_scene_hit(&hit);
            return;
        }
        self.update_decoration_hover_for_scene_hit(&hit);
        if !self.pointer_scene_hit_allowed_by_popup_grab(&hit) {
            self.clear_pointer_focus();
            return;
        }
        let PointerSceneHit::Client { target } = hit else {
            self.focus_desktop_window_at_pointer_scene_hit(&hit);
            self.clear_pointer_focus();
            return;
        };
        self.focus_desktop_window_at_pointer_target(&target);
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

    pub(in crate::compositor) fn send_window_interaction_pointer_motion(
        &mut self,
        timestamp_usec: u64,
        x: f64,
        y: f64,
    ) -> usize {
        let Some((interaction_id, root_surface_id, pointer_motion_surface_id)) =
            self.window_interaction.map(|interaction| {
                (
                    interaction.id,
                    interaction.root_surface_id,
                    interaction.pointer_motion_surface_id,
                )
            })
        else {
            return 0;
        };
        let Some(surface_id) = pointer_motion_surface_id else {
            pointer_debug_log(format!(
                "pointer.interaction_motion interaction={} target=none dispatched=0 relative_suppressed=true",
                interaction_id.get(),
            ));
            return 0;
        };
        let Some(surface) = self.surface_resource_by_id(surface_id) else {
            pointer_debug_log(format!(
                "pointer.interaction_motion interaction={} target={} dispatched=0 reason=surface-missing relative_suppressed=true",
                interaction_id.get(),
                surface_id,
            ));
            return 0;
        };
        if !surface.is_alive() || self.root_surface_id_for_surface(surface_id) != root_surface_id {
            pointer_debug_log(format!(
                "pointer.interaction_motion interaction={} target={} dispatched=0 reason=target-not-owned relative_suppressed=true",
                interaction_id.get(),
                surface_id,
            ));
            return 0;
        }
        let Some(target) = self.pointer_target_for_grabbed_surface_at_output(&surface, x, y) else {
            pointer_debug_log(format!(
                "pointer.interaction_motion interaction={} target={} dispatched=0 reason=surface-not-renderable relative_suppressed=true",
                interaction_id.get(),
                surface_id,
            ));
            return 0;
        };

        self.last_pointer_motion_usec = Some(timestamp_usec);
        let time = wayland_event_time();
        self.pointer_resources.retain(Resource::is_alive);
        let pointers = self
            .pointer_resources
            .iter()
            .filter(|pointer| {
                resource_belongs_to_surface_client(*pointer, &target.surface)
                    && self.pointer_resource_entered_surface(pointer, &target.surface)
            })
            .cloned()
            .collect::<Vec<_>>();
        let dispatched = pointers.len();
        for pointer in pointers {
            let _ = pointer.send_event(wl_pointer::Event::Motion {
                time,
                surface_x: target.surface_x,
                surface_y: target.surface_y,
            });
            send_pointer_frame_if_supported(&pointer);
        }
        pointer_debug_log(format!(
            "pointer.interaction_motion interaction={} root={} target={} output=({x},{y}) local=({},{}) dispatched={} relative_suppressed=true",
            interaction_id.get(),
            root_surface_id,
            surface_id,
            target.surface_x,
            target.surface_y,
            dispatched,
        ));
        dispatched
    }

    pub(in crate::compositor) fn update_pointer_position(&mut self, x: f64, y: f64) {
        self.update_pointer_position_state(x, y);
        self.update_decoration_hover();
    }

    fn update_pointer_position_state(&mut self, x: f64, y: f64) {
        let changed = self.last_pointer_x != x || self.last_pointer_y != y;
        let moves_visible_cursor = changed
            && (self.interaction_cursor_override.is_some()
                || self.client_cursor_render_state().is_some());
        self.last_pointer_x = x;
        self.last_pointer_y = y;
        if moves_visible_cursor {
            self.advance_cursor_generation();
        }
    }

    pub(in crate::compositor) fn update_pointer_position_without_client_dispatch(
        &mut self,
        x: f64,
        y: f64,
    ) -> bool {
        let changed = self.last_pointer_x != x || self.last_pointer_y != y;
        let moves_visible_cursor = changed
            && (self.interaction_cursor_override.is_some()
                || self.client_cursor_render_state().is_some());
        self.update_pointer_position(x, y);
        moves_visible_cursor
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
