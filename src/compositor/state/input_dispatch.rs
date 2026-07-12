use super::*;

impl CompositorState {
    pub(in crate::compositor) fn add_idle_inhibitor(
        &mut self,
        inhibitor: zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1,
    ) {
        self.idle_inhibitor_resources.push(inhibitor);
        self.idle_manager.inhibit();
    }

    pub(in crate::compositor) fn remove_idle_inhibitor(
        &mut self,
        inhibitor: &zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1,
    ) {
        let before = self.idle_inhibitor_resources.len();
        self.idle_inhibitor_resources
            .retain(|resource| !same_wayland_resource(resource, inhibitor));
        if self.idle_inhibitor_resources.len() != before {
            self.idle_manager.uninhibit();
        }
    }

    pub fn idle_inhibited(&mut self) -> bool {
        self.idle_inhibitor_resources.retain(Resource::is_alive);
        if self.idle_inhibitor_resources.is_empty() {
            while self.idle_manager.is_inhibited() {
                self.idle_manager.uninhibit();
            }
        }
        self.idle_manager.is_inhibited()
    }

    pub(in crate::compositor) fn add_relative_pointer_resource(
        &mut self,
        pointer: zwp_relative_pointer_v1::ZwpRelativePointerV1,
        source_pointer: wl_pointer::WlPointer,
    ) {
        pointer_debug_log(format!(
            "pointer.relative create relative={} source_pointer={} client={}",
            pointer.id().protocol_id(),
            source_pointer.id().protocol_id(),
            wayland_resource_client_label(&source_pointer)
        ));
        self.relative_pointer_resources
            .push(RelativePointerResource {
                resource: pointer,
                source_pointer,
            });
    }

    pub(in crate::compositor) fn remove_relative_pointer_resource(
        &mut self,
        pointer: &zwp_relative_pointer_v1::ZwpRelativePointerV1,
    ) {
        pointer_debug_log(format!(
            "pointer.relative destroy relative={} client={}",
            pointer.id().protocol_id(),
            wayland_resource_client_label(pointer)
        ));
        self.relative_pointer_resources
            .retain(|resource| !same_wayland_resource(&resource.resource, pointer));
    }

    pub(in crate::compositor) fn send_relative_pointer_motion(
        &mut self,
        timestamp_usec: u64,
        motion: RelativePointerMotion,
    ) {
        if motion.is_zero() {
            return;
        }
        self.relative_pointer_resources
            .retain(|resource| resource.resource.is_alive() && resource.source_pointer.is_alive());
        let live_relative_count = self.relative_pointer_resources.len();
        if let Some(active) = self.active_locked_pointer_binding() {
            self.pin_locked_pointer_focus(&active);
            self.dispatch_locked_relative_pointer_motion(
                timestamp_usec,
                motion,
                &active,
                live_relative_count,
            );
            return;
        }

        let Some(surface) = self.pointer_surface.clone() else {
            self.relative_motion_debug.note_drop(format!(
                "no pointer focus; active_lock=absent relative_resources={live_relative_count}"
            ));
            return;
        };
        let dispatch_count = self.dispatch_relative_pointer_motion_to_surface_client(
            timestamp_usec,
            motion,
            &surface,
        );
        if dispatch_count == 0 {
            self.relative_motion_debug.note_drop(format!(
                "unlocked route found no recipient; pointer_surface={} client={} relative_resources={live_relative_count}",
                compositor_surface_id(&surface),
                wayland_resource_client_label(&surface)
            ));
        }
    }

    pub(in crate::compositor) fn dispatch_locked_relative_pointer_motion(
        &mut self,
        timestamp_usec: u64,
        motion: RelativePointerMotion,
        active: &ActiveLockedPointerRouting,
        live_relative_count: usize,
    ) {
        let utime_hi = (timestamp_usec >> 32) as u32;
        let utime_lo = (timestamp_usec & 0xffff_ffff) as u32;
        let pointer_entered =
            self.pointer_resource_entered_surface(&active.pointer, &active.surface);
        let relative_pointers = self.relative_pointer_resources.clone();
        let mut recipients: Vec<RelativePointerResource> = Vec::new();
        let mut exact_source_pointer_count = 0usize;
        let mut same_client_count = 0usize;
        let mut same_seat_count = 0usize;
        let mut stale_count = 0usize;
        let mut cross_client_count = 0usize;

        for relative_pointer in relative_pointers {
            if !relative_pointer.resource.is_alive() || !relative_pointer.source_pointer.is_alive()
            {
                stale_count += 1;
                continue;
            }
            if !resource_belongs_to_surface_client(&relative_pointer.resource, &active.surface)
                || !resource_belongs_to_surface_client(
                    &relative_pointer.source_pointer,
                    &active.surface,
                )
            {
                cross_client_count += 1;
                continue;
            }
            same_client_count += 1;
            // Typhon currently exposes a single wl_seat. Exact wl_pointer
            // resource equality is too strict because clients may create
            // constraints and relative-pointer resources from different
            // wl_pointer objects on the same client seat. When multi-seat
            // support is added, store and compare an explicit seat id here.
            same_seat_count += 1;
            if same_wayland_resource(&relative_pointer.source_pointer, &active.pointer) {
                exact_source_pointer_count += 1;
            }
            if !recipients.iter().any(|recipient| {
                same_wayland_resource(&recipient.resource, &relative_pointer.resource)
            }) {
                recipients.push(relative_pointer);
            }
        }

        let selected_recipient_count = recipients.len();

        if self.relative_motion_debug.should_log_route_snapshot() {
            let relative_sources = self
                .relative_pointer_resources
                .iter()
                .map(|relative_pointer| {
                    format!(
                        "relative={} source_pointer={} source_client={} source_seat=untracked",
                        relative_pointer.resource.id().protocol_id(),
                        relative_pointer.source_pointer.id().protocol_id(),
                        wayland_resource_client_label(&relative_pointer.source_pointer)
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            pointer_debug_log(format!(
                "relative route snapshot constraint={} generation={} surface={} surface_client={} lock_pointer={} lock_client={} lock_seat=single exact_source_pointer_count={} same_client_count={} same_seat_count={} selected_recipient_count={} pointer_entered={} live_relative_count={} stale_count={} cross_client_count={} [{}]",
                active.constraint_id,
                active.generation,
                compositor_surface_id(&active.surface),
                wayland_resource_client_label(&active.surface),
                active.pointer.id().protocol_id(),
                wayland_resource_client_label(&active.pointer),
                exact_source_pointer_count,
                same_client_count,
                same_seat_count,
                selected_recipient_count,
                pointer_entered,
                live_relative_count,
                stale_count,
                cross_client_count,
                relative_sources
            ));
        }

        let dispatched_ids = recipients
            .iter()
            .map(|relative_pointer| relative_pointer.resource.id().protocol_id())
            .collect::<Vec<_>>();
        pointer_debug_log(format!(
            "relative route exact_source_pointer_count={} same_client_count={} same_seat_count={} selected_recipient_count={} dispatched={:?} client={} seat=single constraint={} generation={}",
            exact_source_pointer_count,
            same_client_count,
            same_seat_count,
            selected_recipient_count,
            dispatched_ids,
            wayland_resource_client_label(&active.surface),
            active.constraint_id,
            active.generation
        ));

        let mut frame_pointers: Vec<wl_pointer::WlPointer> = Vec::new();
        let mut relative_events_sent = 0usize;
        for relative_pointer in recipients {
            relative_pointer.resource.relative_motion(
                utime_hi,
                utime_lo,
                motion.dx,
                motion.dy,
                motion.dx_unaccelerated,
                motion.dy_unaccelerated,
            );
            relative_events_sent += 1;
            if relative_pointer.source_pointer.is_alive()
                && !frame_pointers
                    .iter()
                    .any(|pointer| same_wayland_resource(pointer, &relative_pointer.source_pointer))
            {
                frame_pointers.push(relative_pointer.source_pointer.clone());
            }
            self.relative_motion_debug.note_dispatch(format!(
                "relative motion dispatched constraint={} generation={} pointer={} source_pointer={} relative={} dx={} dy={}",
                active.constraint_id,
                active.generation,
                active.pointer.id().protocol_id(),
                relative_pointer.source_pointer.id().protocol_id(),
                relative_pointer.resource.id().protocol_id(),
                motion.dx,
                motion.dy
            ));
        }
        let source_pointer_ids = frame_pointers
            .iter()
            .map(|pointer| pointer.id().protocol_id())
            .collect::<Vec<_>>();
        let unique_source_pointer_count = frame_pointers.len();
        let mut pointer_frames_sent = 0usize;
        for pointer in frame_pointers {
            if pointer.is_alive() {
                send_pointer_frame_if_supported(&pointer);
                pointer_frames_sent += 1;
            }
        }
        pointer_debug_log(format!(
            "pointer.relative locked_dispatch constraint={} generation={} selected_recipient_count={} unique_source_pointer_count={} relative_events_sent={} pointer_frames_sent={} source_pointers={:?}",
            active.constraint_id,
            active.generation,
            selected_recipient_count,
            unique_source_pointer_count,
            relative_events_sent,
            pointer_frames_sent,
            source_pointer_ids
        ));
        if relative_events_sent == 0 {
            let reason = if same_client_count > 0 {
                format!(
                    "locked route rejected all same-client relative pointers; constraint={} generation={} pointer={} client={} surface={} client={} exact_source_pointer_count={} same_client_count={} same_seat_count={} selected_recipient_count={} stale_count={} cross_client_count={} pointer_entered={pointer_entered} relative_resources={live_relative_count}",
                    active.constraint_id,
                    active.generation,
                    active.pointer.id().protocol_id(),
                    wayland_resource_client_label(&active.pointer),
                    compositor_surface_id(&active.surface),
                    wayland_resource_client_label(&active.surface),
                    exact_source_pointer_count,
                    same_client_count,
                    same_seat_count,
                    selected_recipient_count,
                    stale_count,
                    cross_client_count,
                )
            } else {
                format!(
                    "locked route has no same-client relative pointer; constraint={} generation={} pointer={} client={} surface={} client={} exact_source_pointer_count={} same_client_count=0 same_seat_count=0 selected_recipient_count=0 stale_count={} cross_client_count={} pointer_entered={pointer_entered} relative_resources={live_relative_count}",
                    active.constraint_id,
                    active.generation,
                    active.pointer.id().protocol_id(),
                    wayland_resource_client_label(&active.pointer),
                    compositor_surface_id(&active.surface),
                    wayland_resource_client_label(&active.surface),
                    exact_source_pointer_count,
                    stale_count,
                    cross_client_count,
                )
            };
            self.relative_motion_debug.note_drop(reason);
        }
    }

    pub(in crate::compositor) fn dispatch_relative_pointer_motion_to_surface_client(
        &mut self,
        timestamp_usec: u64,
        motion: RelativePointerMotion,
        surface: &wl_surface::WlSurface,
    ) -> usize {
        let utime_hi = (timestamp_usec >> 32) as u32;
        let utime_lo = (timestamp_usec & 0xffff_ffff) as u32;
        let relative_pointers = self.relative_pointer_resources.clone();
        let mut dispatched_resource_ids = HashSet::new();
        for relative_pointer in relative_pointers {
            if !relative_pointer.resource.is_alive() || !relative_pointer.source_pointer.is_alive()
            {
                continue;
            }
            if !resource_belongs_to_surface_client(&relative_pointer.resource, surface) {
                continue;
            }
            let resource_id = relative_pointer.resource.id().protocol_id();
            if !dispatched_resource_ids.insert(resource_id) {
                continue;
            }
            relative_pointer.resource.relative_motion(
                utime_hi,
                utime_lo,
                motion.dx,
                motion.dy,
                motion.dx_unaccelerated,
                motion.dy_unaccelerated,
            );
            self.relative_motion_debug.note_dispatch(format!(
                "relative motion dispatched client={} relative={} dx={} dy={}",
                wayland_resource_client_label(surface),
                resource_id,
                motion.dx,
                motion.dy
            ));
        }
        dispatched_resource_ids.len()
    }

    pub(in crate::compositor) fn remember_held_pointer_button(&mut self, press: PointerPress) {
        if self
            .held_pointer_buttons
            .iter()
            .any(|held| held.button == press.button)
        {
            pointer_debug_log(format!(
                "duplicate button press ignored button={}",
                press.button
            ));
            return;
        }
        pointer_debug_log(format!(
            "button press button={} surface={} held_count={}",
            press.button,
            compositor_surface_id(&press.surface),
            self.held_pointer_buttons.len() + 1
        ));
        self.held_pointer_buttons.push(press);
    }

    pub(in crate::compositor) fn forget_held_pointer_button(&mut self, button: u32) {
        let before = self.held_pointer_buttons.len();
        self.held_pointer_buttons
            .retain(|held| held.button != button);
        if before == self.held_pointer_buttons.len() {
            pointer_debug_log(format!("unmatched button release ignored button={button}"));
        } else {
            pointer_debug_log(format!(
                "button release button={} held_count={}",
                button,
                self.held_pointer_buttons.len()
            ));
        }
    }

    pub(in crate::compositor) fn implicit_pointer_grab_surface(
        &mut self,
        reason: &'static str,
    ) -> Option<wl_surface::WlSurface> {
        let grab = self.implicit_pointer_grab.clone()?;
        let surface_id = compositor_surface_id(&grab.surface);
        let mapped = self
            .renderable_surfaces
            .iter()
            .any(|renderable| renderable.surface_id == surface_id);
        if !grab.surface.is_alive() || !mapped {
            self.cancel_implicit_pointer_grab_for_surface_ids(&[surface_id], reason);
            return None;
        }
        Some(grab.surface)
    }

    pub(in crate::compositor) fn begin_implicit_pointer_grab(&mut self, press: &PointerPress) {
        if self.implicit_pointer_grab.is_some() {
            return;
        }
        self.implicit_pointer_grab = Some(ImplicitPointerGrab {
            surface: press.surface.clone(),
            root_surface_id: press.root_surface_id,
        });
        pointer_debug_log(format!(
            "implicit grab begin surface={} button={}",
            compositor_surface_id(&press.surface),
            press.button
        ));
    }

    pub(in crate::compositor) fn end_implicit_pointer_grab(&mut self, reason: &'static str) {
        let Some(grab) = self.implicit_pointer_grab.take() else {
            return;
        };
        pointer_debug_log(format!(
            "implicit grab end surface={} reason={}",
            compositor_surface_id(&grab.surface),
            reason
        ));
    }

    pub(in crate::compositor) fn cancel_implicit_pointer_grab_for_surface_ids(
        &mut self,
        surface_ids: &[u32],
        reason: &'static str,
    ) {
        let Some(grab) = self.implicit_pointer_grab.as_ref() else {
            return;
        };
        let grab_surface_id = compositor_surface_id(&grab.surface);
        if !surface_ids.contains(&grab_surface_id) && !surface_ids.contains(&grab.root_surface_id) {
            return;
        }
        self.end_implicit_pointer_grab(reason);
        self.held_pointer_buttons.retain(|press| {
            !surface_ids.contains(&compositor_surface_id(&press.surface))
                && !surface_ids.contains(&press.root_surface_id)
        });
        if self.last_pointer_press.as_ref().is_some_and(|press| {
            surface_ids.contains(&compositor_surface_id(&press.surface))
                || surface_ids.contains(&press.root_surface_id)
        }) {
            self.last_pointer_press = None;
        }
    }

    pub(in crate::compositor) fn pointer_target_for_grabbed_surface_at_output(
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
        let mut origin = self.surface_origin_cache.get(index).copied()?;
        if let Some(interaction) = self.window_interaction
            && let Some(pending) = self.pending_interactive_resize_update
            && pending.root_surface_id == interaction.root_surface_id
            && self.root_surface_id_for_surface(surface_id) == interaction.root_surface_id
        {
            let current_placement = self
                .current_visual_root_window_geometry(interaction.root_surface_id)
                .map(|geometry| geometry.placement)
                .unwrap_or_else(|| self.surface_placement(interaction.root_surface_id));
            origin.0 = origin
                .0
                .saturating_add(pending.placement.local_x - current_placement.local_x);
            origin.1 = origin
                .1
                .saturating_add(pending.placement.local_y - current_placement.local_y);
        }
        Some(PointerTarget {
            surface: surface.clone(),
            surface_x: x - f64::from(origin.0),
            surface_y: y - f64::from(origin.1),
        })
    }

    pub(in crate::compositor) fn send_implicit_pointer_grab_motion(
        &mut self,
        x: f64,
        y: f64,
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
            "implicit grab motion surface={} output=({},{}) local=({},{})",
            compositor_surface_id(&surface),
            x,
            y,
            target.surface_x,
            target.surface_y
        ));
        let time = wayland_event_time();
        for pointer in self
            .pointer_resources
            .iter()
            .filter(|pointer| resource_belongs_to_surface_client(*pointer, &surface))
        {
            let _ = pointer.send_event(wl_pointer::Event::Motion {
                time,
                surface_x: target.surface_x,
                surface_y: target.surface_y,
            });
            send_pointer_frame_if_supported(pointer);
        }
        true
    }

    pub(in crate::compositor) fn send_pointer_button(&mut self, button: u32, pressed: bool) {
        if let Some(locked_surface) = self.locked_pointer_input_surface() {
            self.ensure_pointer_focus(&locked_surface);
            if let Some(active) = self.active_locked_pointer_binding() {
                self.pin_locked_pointer_focus(&active);
            }
            let surface = locked_surface;
            let state = if pressed {
                wl_pointer::ButtonState::Pressed
            } else {
                wl_pointer::ButtonState::Released
            };
            let serial = self.next_configure_serial();
            let time = wayland_event_time();
            self.remember_input_serial(serial, surface.clone());
            if pressed {
                let surface_id = compositor_surface_id(&surface);
                let root_surface_id = self.root_surface_id_for_surface(surface_id);
                if self
                    .topmost_popup_grab_surface_id()
                    .is_some_and(|popup_id| self.surface_is_descendant_of(surface_id, popup_id))
                {
                    self.focus_surface(surface.clone());
                } else if let Some(root_surface) = self.surface_resource_by_id(root_surface_id) {
                    self.focus_surface(root_surface);
                }
                let press = PointerPress {
                    serial,
                    button,
                    surface: surface.clone(),
                    root_surface_id,
                    output_x: self.last_pointer_x,
                    output_y: self.last_pointer_y,
                };
                self.remember_held_pointer_button(press.clone());
                self.last_pointer_press = Some(press);
            } else if self
                .last_pointer_press
                .as_ref()
                .is_some_and(|press| press.button == button)
            {
                self.forget_held_pointer_button(button);
                self.last_pointer_press = None;
            } else {
                self.forget_held_pointer_button(button);
            }
            if !pressed
                && self.held_pointer_buttons.is_empty()
                && self.implicit_pointer_grab.is_some()
            {
                self.end_implicit_pointer_grab("last-release");
            }
            for pointer in self
                .pointer_resources
                .iter()
                .filter(|pointer| resource_belongs_to_surface_client(*pointer, &surface))
            {
                let _ = pointer.send_event(wl_pointer::Event::Button {
                    serial,
                    time,
                    button,
                    state: WEnum::Value(state),
                });
                send_pointer_frame_if_supported(pointer);
            }
            return;
        }

        let grabbed_surface = self.implicit_pointer_grab_surface("surface-destroyed");
        let target = if grabbed_surface.is_none() {
            self.pointer_target_at(self.last_pointer_x, self.last_pointer_y)
        } else {
            None
        };
        if grabbed_surface.is_none() {
            if pressed
                && let Some(popup_surface_id) =
                    self.popup_grab_to_dismiss_for_pointer_target(target.as_ref())
            {
                self.dismiss_popup_surface(popup_surface_id);
                let _ = self.focus_topmost_renderable_toplevel();
                return;
            }

            if let Some(target) = target.as_ref() {
                self.ensure_pointer_focus(&target.surface);
                self.send_pointer_enter_if_needed(target);
            }
        }

        let Some(surface) = grabbed_surface
            .or_else(|| {
                (!pressed).then(|| {
                    self.last_pointer_press
                        .as_ref()
                        .filter(|press| press.button == button)
                        .map(|press| press.surface.clone())
                })?
            })
            .or_else(|| target.map(|target| target.surface))
            .or_else(|| self.pointer_surface.clone())
            .or_else(|| self.focused_surface.clone())
        else {
            return;
        };
        let state = if pressed {
            wl_pointer::ButtonState::Pressed
        } else {
            wl_pointer::ButtonState::Released
        };
        let serial = self.next_configure_serial();
        let time = wayland_event_time();
        self.remember_input_serial(serial, surface.clone());

        if pressed {
            let surface_id = compositor_surface_id(&surface);
            let root_surface_id = self.root_surface_id_for_surface(surface_id);
            if self
                .topmost_popup_grab_surface_id()
                .is_some_and(|popup_id| self.surface_is_descendant_of(surface_id, popup_id))
            {
                self.focus_surface(surface.clone());
            } else if self.layer_surfaces.contains_key(&root_surface_id) {
                let _ = self.activate_ondemand_layer_surface(root_surface_id);
            } else if let Some(root_surface) = self.surface_resource_by_id(root_surface_id) {
                self.focus_surface(root_surface);
            }
            let press = PointerPress {
                serial,
                button,
                surface: surface.clone(),
                root_surface_id,
                output_x: self.last_pointer_x,
                output_y: self.last_pointer_y,
            };
            let was_empty = self.held_pointer_buttons.is_empty();
            self.remember_held_pointer_button(press.clone());
            if was_empty
                && self
                    .held_pointer_buttons
                    .iter()
                    .any(|held| held.button == button)
            {
                self.begin_implicit_pointer_grab(&press);
            }
            self.last_pointer_press = Some(press);
        } else if self
            .last_pointer_press
            .as_ref()
            .is_some_and(|press| press.button == button)
        {
            self.forget_held_pointer_button(button);
            self.last_pointer_press = None;
        } else {
            self.forget_held_pointer_button(button);
        }

        for pointer in self
            .pointer_resources
            .iter()
            .filter(|pointer| resource_belongs_to_surface_client(*pointer, &surface))
        {
            let _ = pointer.send_event(wl_pointer::Event::Button {
                serial,
                time,
                button,
                state: WEnum::Value(state),
            });
            send_pointer_frame_if_supported(pointer);
        }
        pointer_debug_log(format!(
            "implicit grab button surface={} button={} state={} held={}",
            compositor_surface_id(&surface),
            button,
            if pressed { "pressed" } else { "released" },
            self.held_pointer_buttons.len()
        ));
        if !pressed && self.held_pointer_buttons.is_empty() && self.implicit_pointer_grab.is_some()
        {
            let old_surface_id = self
                .implicit_pointer_grab
                .as_ref()
                .map(|grab| compositor_surface_id(&grab.surface));
            self.end_implicit_pointer_grab("last-release");
            self.refresh_pointer_focus_after_implicit_grab(old_surface_id);
        }
    }

    pub(in crate::compositor) fn send_pointer_axis(&mut self, horizontal: f64, vertical: f64) {
        if horizontal == 0.0 && vertical == 0.0 {
            return;
        }

        if let Some(surface) = self.locked_pointer_input_surface() {
            if let Some(active) = self.active_locked_pointer_binding() {
                self.pin_locked_pointer_focus(&active);
            }
            self.ensure_pointer_focus(&surface);
            let time = wayland_event_time();
            for pointer in self
                .pointer_resources
                .iter()
                .filter(|pointer| resource_belongs_to_surface_client(*pointer, &surface))
            {
                if horizontal != 0.0 {
                    let _ = pointer.send_event(wl_pointer::Event::Axis {
                        time,
                        axis: WEnum::Value(wl_pointer::Axis::HorizontalScroll),
                        value: horizontal,
                    });
                }
                if vertical != 0.0 {
                    let _ = pointer.send_event(wl_pointer::Event::Axis {
                        time,
                        axis: WEnum::Value(wl_pointer::Axis::VerticalScroll),
                        value: vertical,
                    });
                }
                send_pointer_frame_if_supported(pointer);
            }
            return;
        }

        if let Some(surface) = self.implicit_pointer_grab_surface("surface-destroyed") {
            let time = wayland_event_time();
            for pointer in self
                .pointer_resources
                .iter()
                .filter(|pointer| resource_belongs_to_surface_client(*pointer, &surface))
            {
                if horizontal != 0.0 {
                    let _ = pointer.send_event(wl_pointer::Event::Axis {
                        time,
                        axis: WEnum::Value(wl_pointer::Axis::HorizontalScroll),
                        value: horizontal,
                    });
                }
                if vertical != 0.0 {
                    let _ = pointer.send_event(wl_pointer::Event::Axis {
                        time,
                        axis: WEnum::Value(wl_pointer::Axis::VerticalScroll),
                        value: vertical,
                    });
                }
                send_pointer_frame_if_supported(pointer);
            }
            return;
        }

        let Some(target) = self.pointer_target_at(self.last_pointer_x, self.last_pointer_y) else {
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
            if horizontal != 0.0 {
                let _ = pointer.send_event(wl_pointer::Event::Axis {
                    time,
                    axis: WEnum::Value(wl_pointer::Axis::HorizontalScroll),
                    value: horizontal,
                });
            }
            if vertical != 0.0 {
                let _ = pointer.send_event(wl_pointer::Event::Axis {
                    time,
                    axis: WEnum::Value(wl_pointer::Axis::VerticalScroll),
                    value: vertical,
                });
            }
            send_pointer_frame_if_supported(pointer);
        }
    }
}
