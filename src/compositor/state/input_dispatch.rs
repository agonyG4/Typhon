use super::*;

impl CompositorState {
    pub(in crate::compositor) fn refresh_input_serial_focus_generation(&mut self, serial: u32) {
        let focus_generation = self.focus_generation;
        if let Some(input) = self
            .recent_input_serials
            .iter_mut()
            .find(|input| input.serial == serial)
        {
            input.focus_generation = focus_generation;
        }
    }

    pub(in crate::compositor) fn add_idle_inhibitor(
        &mut self,
        inhibitor: zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1,
        client_id: ClientId,
        target_surface: wl_surface::WlSurface,
    ) {
        self.idle_inhibitor_resources.push(IdleInhibitorBinding {
            inhibitor,
            client_id,
            target_surface,
        });
        self.reconcile_idle_inhibition();
    }

    pub(in crate::compositor) fn remove_idle_inhibitor(
        &mut self,
        inhibitor: &zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1,
    ) {
        self.idle_inhibitor_resources
            .retain(|binding| !same_wayland_resource(&binding.inhibitor, inhibitor));
        self.reconcile_idle_inhibition();
    }

    pub fn idle_inhibited(&mut self) -> bool {
        self.reconcile_idle_inhibition();
        self.idle_manager.is_inhibited()
    }

    pub(in crate::compositor) fn reconcile_idle_inhibition(&mut self) {
        self.idle_inhibitor_resources
            .retain(|binding| binding.inhibitor.is_alive() && binding.target_surface.is_alive());
        let effective_count = self
            .idle_inhibitor_resources
            .iter()
            .filter(|binding| self.idle_inhibitor_is_effective(binding))
            .count();
        self.idle_manager.reconcile_inhibited_count(effective_count);
    }

    fn idle_inhibitor_is_effective(&self, binding: &IdleInhibitorBinding) -> bool {
        let target_surface_id = compositor_surface_id(&binding.target_surface);
        if self
            .surface_client_ids
            .get(&target_surface_id)
            .is_none_or(|client_id| *client_id != binding.client_id)
        {
            return false;
        }
        if !self.surface_resources.contains_key(&target_surface_id) {
            return false;
        }
        let root_surface_id = self.root_surface_id_for_surface(target_surface_id);
        if !self.surface_is_visible_in_active_workspace(root_surface_id)
            && !self.layer_surfaces.contains_key(&root_surface_id)
        {
            return false;
        }
        if self
            .toplevel_window_state(root_surface_id)
            .is_some_and(WindowState::is_minimized)
        {
            return false;
        }
        self.renderable_surfaces
            .iter()
            .any(|surface| self.root_surface_id_for_surface(surface.surface_id) == root_surface_id)
            || self
                .current_surface_buffers
                .keys()
                .any(|surface_id| self.root_surface_id_for_surface(*surface_id) == root_surface_id)
    }

    pub(in crate::compositor) fn advance_relative_pointer_resources_generation(&mut self) {
        self.relative_pointer_resources_generation = self
            .relative_pointer_resources_generation
            .wrapping_add(1)
            .max(1);
        self.locked_relative_recipient_cache.invalidate();
    }

    pub(in crate::compositor) fn invalidate_locked_relative_recipient_cache(&mut self) {
        self.locked_relative_recipient_cache.invalidate();
    }

    fn retain_live_relative_pointer_resources(&mut self) {
        let before = self.relative_pointer_resources.len();
        self.relative_pointer_resources
            .retain(|resource| resource.resource.is_alive() && resource.source_pointer.is_alive());
        if self.relative_pointer_resources.len() != before {
            self.advance_relative_pointer_resources_generation();
        }
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
        self.advance_relative_pointer_resources_generation();
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
        let before = self.relative_pointer_resources.len();
        self.relative_pointer_resources
            .retain(|resource| !same_wayland_resource(&resource.resource, pointer));
        if self.relative_pointer_resources.len() != before {
            self.advance_relative_pointer_resources_generation();
        }
    }

    pub(in crate::compositor) fn send_relative_pointer_motion(
        &mut self,
        timestamp_usec: u64,
        motion: RelativePointerMotion,
    ) {
        if motion.is_zero() {
            return;
        }
        self.retain_live_relative_pointer_resources();
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
        let cache_key = LockedRelativeRecipientCacheKey {
            resource_generation: self.relative_pointer_resources_generation,
            constraint_generation: active.generation,
            surface_id: compositor_surface_id(&active.surface),
            source_pointer_id: active.pointer.id().protocol_id(),
        };
        if !self.locked_relative_recipient_cache.matches(cache_key) {
            self.rebuild_locked_relative_recipient_cache(cache_key, active);
        }
        let exact_source_pointer_count = self
            .locked_relative_recipient_cache
            .exact_source_pointer_count;
        let same_client_count = self.locked_relative_recipient_cache.same_client_count;
        let same_seat_count = self.locked_relative_recipient_cache.same_seat_count;
        let stale_count = self.locked_relative_recipient_cache.stale_count;
        let cross_client_count = self.locked_relative_recipient_cache.cross_client_count;
        let selected_recipient_count = self.locked_relative_recipient_cache.recipients.len();

        if self.relative_motion_debug.should_log_route_snapshot() {
            pointer_debug_log_lazy(|| {
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
                format!(
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
                )
            });
        }

        pointer_debug_log_lazy(|| {
            let dispatched_ids = self
                .locked_relative_recipient_cache
                .recipients
                .iter()
                .map(|relative_pointer| relative_pointer.resource.id().protocol_id())
                .collect::<Vec<_>>();
            format!(
                "relative route exact_source_pointer_count={} same_client_count={} same_seat_count={} selected_recipient_count={} dispatched={:?} client={} seat=single constraint={} generation={}",
                exact_source_pointer_count,
                same_client_count,
                same_seat_count,
                selected_recipient_count,
                dispatched_ids,
                wayland_resource_client_label(&active.surface),
                active.constraint_id,
                active.generation
            )
        });

        let mut relative_events_sent = 0usize;
        for index in 0..selected_recipient_count {
            let relative_pointer = &self.locked_relative_recipient_cache.recipients[index];
            relative_pointer.resource.relative_motion(
                utime_hi,
                utime_lo,
                motion.dx,
                motion.dy,
                motion.dx_unaccelerated,
                motion.dy_unaccelerated,
            );
            relative_events_sent += 1;
            self.relative_motion_debug.note_dispatch(|| format!(
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
        let unique_source_pointer_count = self.locked_relative_recipient_cache.frame_pointers.len();
        let mut pointer_frames_sent = 0usize;
        for pointer in &self.locked_relative_recipient_cache.frame_pointers {
            if pointer.is_alive() {
                send_pointer_frame_if_supported(pointer);
                pointer_frames_sent += 1;
            }
        }
        pointer_debug_log_lazy(|| {
            format!(
                "pointer.relative locked_dispatch constraint={} generation={} selected_recipient_count={} unique_source_pointer_count={} relative_events_sent={} pointer_frames_sent={}",
                active.constraint_id,
                active.generation,
                selected_recipient_count,
                unique_source_pointer_count,
                relative_events_sent,
                pointer_frames_sent
            )
        });
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

    fn rebuild_locked_relative_recipient_cache(
        &mut self,
        key: LockedRelativeRecipientCacheKey,
        active: &ActiveLockedPointerRouting,
    ) {
        let cache = &mut self.locked_relative_recipient_cache;
        cache.key = None;
        cache.recipients.clear();
        cache.frame_pointers.clear();
        cache.exact_source_pointer_count = 0;
        cache.same_client_count = 0;
        cache.same_seat_count = 0;
        cache.stale_count = 0;
        cache.cross_client_count = 0;

        for relative_pointer in &self.relative_pointer_resources {
            if !relative_pointer.resource.is_alive() || !relative_pointer.source_pointer.is_alive()
            {
                cache.stale_count += 1;
                continue;
            }
            if !resource_belongs_to_surface_client(&relative_pointer.resource, &active.surface)
                || !resource_belongs_to_surface_client(
                    &relative_pointer.source_pointer,
                    &active.surface,
                )
            {
                cache.cross_client_count += 1;
                continue;
            }
            cache.same_client_count += 1;
            // Typhon currently exposes a single wl_seat. Exact wl_pointer
            // resource equality is too strict because clients may create
            // constraints and relative-pointer resources from different
            // wl_pointer objects on the same client seat. When multi-seat
            // support is added, store and compare an explicit seat id here.
            cache.same_seat_count += 1;
            if same_wayland_resource(&relative_pointer.source_pointer, &active.pointer) {
                cache.exact_source_pointer_count += 1;
            }
            if cache.recipients.iter().any(|recipient| {
                same_wayland_resource(&recipient.resource, &relative_pointer.resource)
            }) {
                continue;
            }
            cache.recipients.push(relative_pointer.clone());
            if relative_pointer.source_pointer.is_alive()
                && !cache
                    .frame_pointers
                    .iter()
                    .any(|pointer| same_wayland_resource(pointer, &relative_pointer.source_pointer))
            {
                cache
                    .frame_pointers
                    .push(relative_pointer.source_pointer.clone());
            }
        }
        cache.key = Some(key);
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
            self.relative_motion_debug.note_dispatch(|| {
                format!(
                    "relative motion dispatched client={} relative={} dx={} dy={}",
                    wayland_resource_client_label(surface),
                    resource_id,
                    motion.dx,
                    motion.dy
                )
            });
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

    fn pointer_press_matches_release_context(
        press: &PointerPress,
        context: WindowInteractionReleaseContext,
    ) -> bool {
        press.button == context.trigger_button
            && context
                .trigger_serial
                .is_none_or(|serial| press.serial == serial)
            && context
                .original_surface_id
                .is_none_or(|surface_id| compositor_surface_id(&press.surface) == surface_id)
    }

    fn client_owned_trigger_release_target(
        &self,
        context: WindowInteractionReleaseContext,
    ) -> Option<wl_surface::WlSurface> {
        let original_press = self
            .held_pointer_buttons
            .iter()
            .chain(self.last_pointer_press.iter())
            .find(|press| Self::pointer_press_matches_release_context(press, context));
        if let Some(press) = original_press
            && press.surface.is_alive()
        {
            return Some(press.surface.clone());
        }
        None
    }

    fn has_client_owned_trigger_ownership(&self, context: WindowInteractionReleaseContext) -> bool {
        self.held_pointer_buttons
            .iter()
            .chain(self.last_pointer_press.iter())
            .any(|press| Self::pointer_press_matches_release_context(press, context))
    }

    fn clear_client_owned_trigger_ownership(&mut self, button: u32) -> usize {
        let before = self.held_pointer_buttons.len();
        self.forget_held_pointer_button(button);
        if self
            .last_pointer_press
            .as_ref()
            .is_some_and(|press| press.button == button)
        {
            self.last_pointer_press = None;
        }
        let cleared = before.saturating_sub(self.held_pointer_buttons.len());
        if cleared > 0 {
            self.window_interaction_release_metrics
                .window_interaction_stale_buttons_cleared = self
                .window_interaction_release_metrics
                .window_interaction_stale_buttons_cleared
                .saturating_add(cleared as u64);
        }
        if self.held_pointer_buttons.is_empty() {
            if self.implicit_pointer_grab.is_some() {
                self.end_implicit_pointer_grab("release-target-missing");
            }
            self.refresh_pointer_focus_after_implicit_grab(None);
        }
        cleared
    }

    pub(in crate::compositor) fn send_client_owned_trigger_release(
        &mut self,
        context: WindowInteractionReleaseContext,
    ) -> bool {
        let held_button_count_before = self.held_pointer_buttons.len();
        let implicit_grab_surface_id_before = self
            .implicit_pointer_grab
            .as_ref()
            .map(|grab| compositor_surface_id(&grab.surface));
        let has_ownership = self.has_client_owned_trigger_ownership(context);
        if !has_ownership {
            self.window_interaction_release_metrics
                .window_interaction_duplicate_releases_prevented = self
                .window_interaction_release_metrics
                .window_interaction_duplicate_releases_prevented
                .saturating_add(1);
            self.record_client_owned_window_interaction_release(
                context,
                None,
                held_button_count_before,
                self.held_pointer_buttons.len(),
                implicit_grab_surface_id_before,
                self.implicit_pointer_grab
                    .as_ref()
                    .map(|grab| compositor_surface_id(&grab.surface)),
            );
            return false;
        }

        let target = self.client_owned_trigger_release_target(context);
        let Some(target) = target else {
            self.window_interaction_release_metrics
                .window_interaction_release_target_missing = self
                .window_interaction_release_metrics
                .window_interaction_release_target_missing
                .saturating_add(1);
            self.clear_client_owned_trigger_ownership(context.trigger_button);
            self.record_client_owned_window_interaction_release(
                context,
                None,
                held_button_count_before,
                self.held_pointer_buttons.len(),
                implicit_grab_surface_id_before,
                self.implicit_pointer_grab
                    .as_ref()
                    .map(|grab| compositor_surface_id(&grab.surface)),
            );
            return false;
        };

        let target_surface_id = compositor_surface_id(&target);
        self.send_pointer_release_to_surface(&target, context.trigger_button);
        self.window_interaction_release_metrics
            .window_interaction_client_releases_forwarded = self
            .window_interaction_release_metrics
            .window_interaction_client_releases_forwarded
            .saturating_add(1);
        self.record_client_owned_window_interaction_release(
            context,
            Some(target_surface_id),
            held_button_count_before,
            self.held_pointer_buttons.len(),
            implicit_grab_surface_id_before,
            self.implicit_pointer_grab
                .as_ref()
                .map(|grab| compositor_surface_id(&grab.surface)),
        );
        true
    }

    fn send_pointer_release_to_surface(&mut self, surface: &wl_surface::WlSurface, button: u32) {
        let state = wl_pointer::ButtonState::Released;
        let serial = self.next_configure_serial();
        let time = wayland_event_time();
        self.forget_held_pointer_button(button);
        if self
            .last_pointer_press
            .as_ref()
            .is_some_and(|press| press.button == button)
        {
            self.last_pointer_press = None;
        }
        for pointer in self
            .pointer_resources
            .iter()
            .filter(|pointer| resource_belongs_to_surface_client(*pointer, surface))
        {
            let _ = pointer.send_event(wl_pointer::Event::Button {
                serial,
                time,
                button,
                state: WEnum::Value(state),
            });
            send_pointer_frame_if_supported(pointer);
        }
        if self.held_pointer_buttons.is_empty() && self.implicit_pointer_grab.is_some() {
            let old_surface_id = self
                .implicit_pointer_grab
                .as_ref()
                .map(|grab| compositor_surface_id(&grab.surface));
            self.end_implicit_pointer_grab("last-release");
            self.refresh_pointer_focus_after_implicit_grab(old_surface_id);
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
        if self.active_drag.is_some() {
            if reason == "last-release" {
                self.drop_active_drag();
            } else {
                self.cancel_drag_session(reason);
            }
        }
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
        if let Some(index) = self.active_scene_surface_index(surface_id)
            && let Some(origin) = self.active_scene_surface_origins().get(index).copied()
        {
            self.pointer_hit_metrics.grabbed_target_active_scene_hits = self
                .pointer_hit_metrics
                .grabbed_target_active_scene_hits
                .saturating_add(1);
            let (origin_x, origin_y) =
                self.grabbed_surface_origin_with_pending_resize(surface_id, origin);
            return Some(PointerTarget {
                surface: surface.clone(),
                surface_x: x - f64::from(origin_x),
                surface_y: y - f64::from(origin_y),
            });
        }
        self.pointer_hit_metrics.grabbed_target_global_fallbacks = self
            .pointer_hit_metrics
            .grabbed_target_global_fallbacks
            .saturating_add(1);
        self.refresh_surface_origin_cache();
        let index = self
            .renderable_surfaces
            .iter()
            .position(|renderable| renderable.surface_id == surface_id)?;
        let origin = self.surface_origin_cache.get(index).copied()?;
        let (origin_x, origin_y) =
            self.grabbed_surface_origin_with_pending_resize(surface_id, origin);
        Some(PointerTarget {
            surface: surface.clone(),
            surface_x: x - f64::from(origin_x),
            surface_y: y - f64::from(origin_y),
        })
    }

    fn grabbed_surface_origin_with_pending_resize(
        &self,
        surface_id: u32,
        origin: (i32, i32),
    ) -> (i32, i32) {
        let Some(pending) = self.pending_floating_resize else {
            return origin;
        };
        if self.root_surface_id_for_surface(surface_id) != pending.surface_id {
            return origin;
        }
        let Some(current) = self.current_visual_root_window_geometry(pending.surface_id) else {
            return origin;
        };
        (
            origin.0.saturating_add(
                pending
                    .geometry
                    .placement
                    .local_x
                    .saturating_sub(current.placement.local_x),
            ),
            origin.1.saturating_add(
                pending
                    .geometry
                    .placement
                    .local_y
                    .saturating_sub(current.placement.local_y),
            ),
        )
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
        pointer_debug_log_lazy(|| {
            format!(
                "implicit grab motion surface={} output=({},{}) local=({},{})",
                compositor_surface_id(&surface),
                x,
                y,
                target.surface_x,
                target.surface_y
            )
        });
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
        let ordinary_scene_input = self.locked_pointer_input_surface().is_none()
            && self
                .implicit_pointer_grab_surface("surface-destroyed")
                .is_none()
            && self.topmost_popup_grab_surface_id().is_none();
        let scene_hit = ordinary_scene_input
            .then(|| self.pointer_scene_hit_at(self.last_pointer_x, self.last_pointer_y));
        if ordinary_scene_input
            && self.handle_decoration_button_with_hit(scene_hit.as_ref(), button, pressed)
        {
            return;
        }
        if let Some(locked_surface) = self.locked_pointer_input_surface() {
            crate::xwayland::trace::emit("focus_pointer_button", || {
                crate::xwayland::trace::TraceFields::new()
                    .field("source", "compositor")
                    .field("button", button)
                    .field("pressed", pressed)
                    .field("surface_id", compositor_surface_id(&locked_surface))
            });
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
            if pressed {
                self.remember_input_serial(
                    serial,
                    surface.clone(),
                    InputSerialKind::PointerButtonPress { button },
                );
            }
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
                self.refresh_input_serial_focus_generation(serial);
                let press = PointerPress {
                    serial,
                    button,
                    surface: surface.clone(),
                    root_surface_id,
                    window_id: self.window_id_for_surface(root_surface_id),
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
            match scene_hit.as_ref() {
                Some(hit) => self.pointer_target_from_scene_hit(
                    hit,
                    self.last_pointer_x,
                    self.last_pointer_y,
                ),
                None => self.pointer_target_at(self.last_pointer_x, self.last_pointer_y),
            }
        } else {
            None
        };
        let captured_window_id = target.as_ref().and_then(|target| {
            let root_surface_id =
                self.root_surface_id_for_surface(compositor_surface_id(&target.surface));
            self.window_id_for_surface(root_surface_id)
        });
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
                    self.held_pointer_buttons
                        .iter()
                        .rev()
                        .chain(self.last_pointer_press.iter())
                        .find(|press| press.button == button)
                        .map(|press| press.surface.clone())
                })?
            })
            .or_else(|| {
                pressed
                    .then(|| target.map(|target| target.surface))
                    .flatten()
            })
            .or_else(|| pressed.then(|| self.pointer_surface.clone()).flatten())
            .or_else(|| pressed.then(|| self.focused_surface.clone()).flatten())
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
        if pressed {
            self.remember_input_serial(
                serial,
                surface.clone(),
                InputSerialKind::PointerButtonPress { button },
            );
        }

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
            } else if let Some(window_id) = captured_window_id {
                let _ = self.activate_desktop_window(window_id, WindowFocusReason::PointerPress);
            } else if self
                .window_id_for_surface(root_surface_id)
                .and_then(|window_id| self.window(window_id))
                .is_none_or(|window| window.is_normal_x11_role())
                && let Some(root_surface) = self.surface_resource_by_id(root_surface_id)
            {
                self.set_desktop_focus(root_surface, WindowFocusReason::PointerPress.label());
            }
            self.refresh_input_serial_focus_generation(serial);
            let press = PointerPress {
                serial,
                button,
                surface: surface.clone(),
                root_surface_id,
                window_id: captured_window_id,
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
        self.send_pointer_axis_frame(PointerAxisFrame::unknown(
            u64::from(wayland_event_time()) * 1_000,
            horizontal,
            vertical,
        ));
    }

    pub(in crate::compositor) fn send_pointer_axis_frame(&mut self, frame: PointerAxisFrame) {
        let horizontal_empty = frame.horizontal.continuous == Some(0.0)
            && frame.horizontal.value120.is_none()
            && frame.horizontal.discrete.is_none()
            && !frame.horizontal.stopped;
        let vertical_empty = frame.vertical.continuous == Some(0.0)
            && frame.vertical.value120.is_none()
            && frame.vertical.discrete.is_none()
            && !frame.vertical.stopped;
        if horizontal_empty && vertical_empty {
            return;
        }
        self.compliance_metrics.pointer_axis_frames = self
            .compliance_metrics
            .pointer_axis_frames
            .saturating_add(1);

        if let Some(surface) = self.locked_pointer_input_surface() {
            if let Some(active) = self.active_locked_pointer_binding() {
                self.pin_locked_pointer_focus(&active);
            }
            self.ensure_pointer_focus(&surface);
            for pointer in self
                .pointer_resources
                .iter()
                .filter(|pointer| resource_belongs_to_surface_client(*pointer, &surface))
            {
                send_pointer_axis_frame_to_resource(pointer, frame);
            }
            return;
        }

        if let Some(surface) = self.implicit_pointer_grab_surface("surface-destroyed") {
            for pointer in self
                .pointer_resources
                .iter()
                .filter(|pointer| resource_belongs_to_surface_client(*pointer, &surface))
            {
                send_pointer_axis_frame_to_resource(pointer, frame);
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
        self.ensure_pointer_focus(&target.surface);
        self.send_pointer_enter_if_needed(&target);

        for pointer in self
            .pointer_resources
            .iter()
            .filter(|pointer| resource_belongs_to_surface_client(*pointer, &target.surface))
        {
            send_pointer_axis_frame_to_resource(pointer, frame);
        }
    }
}

fn send_pointer_axis_frame_to_resource(pointer: &wl_pointer::WlPointer, frame: PointerAxisFrame) {
    let time = wayland_event_time_from_usec(frame.timestamp_usec);
    if pointer.version() >= 5 {
        let source = match frame.source {
            PointerAxisSource::Wheel => Some(wl_pointer::AxisSource::Wheel),
            PointerAxisSource::Finger => Some(wl_pointer::AxisSource::Finger),
            PointerAxisSource::Continuous => Some(wl_pointer::AxisSource::Continuous),
            PointerAxisSource::WheelTilt if pointer.version() >= 6 => {
                Some(wl_pointer::AxisSource::WheelTilt)
            }
            PointerAxisSource::WheelTilt | PointerAxisSource::Unknown => None,
        };
        if let Some(source) = source {
            let _ = pointer.send_event(wl_pointer::Event::AxisSource {
                axis_source: WEnum::Value(source),
            });
        }
    }

    let axes = [
        (wl_pointer::Axis::HorizontalScroll, frame.horizontal),
        (wl_pointer::Axis::VerticalScroll, frame.vertical),
    ];
    for (axis, component) in axes {
        if pointer.version() >= 8 {
            if let Some(value120) = component.value120.filter(|value120| *value120 != 0) {
                let _ = pointer.send_event(wl_pointer::Event::AxisValue120 {
                    axis: WEnum::Value(axis),
                    value120,
                });
            }
        } else if pointer.version() >= 5
            && let Some(discrete) = component.discrete
        {
            let _ = pointer.send_event(wl_pointer::Event::AxisDiscrete {
                axis: WEnum::Value(axis),
                discrete,
            });
        }
    }
    for (axis, component) in axes {
        if let Some(value) = component.continuous
            && value != 0.0
        {
            let _ = pointer.send_event(wl_pointer::Event::Axis {
                time,
                axis: WEnum::Value(axis),
                value,
            });
        }
    }
    for (axis, component) in axes {
        if pointer.version() >= 5 && component.stopped {
            let _ = pointer.send_event(wl_pointer::Event::AxisStop {
                time,
                axis: WEnum::Value(axis),
            });
        }
    }
    send_pointer_frame_if_supported(pointer);
}

#[cfg(test)]
mod locked_relative_recipient_cache_tests {
    use super::*;

    #[test]
    fn cache_key_requires_all_locked_relative_lifecycle_identities() {
        let key = LockedRelativeRecipientCacheKey {
            resource_generation: 7,
            constraint_generation: 11,
            surface_id: 13,
            source_pointer_id: 17,
        };
        let cache = LockedRelativeRecipientCache {
            key: Some(key),
            ..Default::default()
        };

        assert!(cache.matches(key));
        assert!(!cache.matches(LockedRelativeRecipientCacheKey {
            resource_generation: 8,
            ..key
        }));
        assert!(!cache.matches(LockedRelativeRecipientCacheKey {
            constraint_generation: 12,
            ..key
        }));
        assert!(!cache.matches(LockedRelativeRecipientCacheKey {
            surface_id: 14,
            ..key
        }));
        assert!(!cache.matches(LockedRelativeRecipientCacheKey {
            source_pointer_id: 18,
            ..key
        }));
    }

    #[test]
    fn resource_lifetime_invalidation_keeps_warm_cache_capacity() {
        let key = LockedRelativeRecipientCacheKey {
            resource_generation: 3,
            constraint_generation: 5,
            surface_id: 7,
            source_pointer_id: 9,
        };
        let mut cache = LockedRelativeRecipientCache {
            key: Some(key),
            recipients: Vec::with_capacity(4),
            frame_pointers: Vec::with_capacity(2),
            ..Default::default()
        };
        let recipient_capacity = cache.recipients.capacity();
        let frame_capacity = cache.frame_pointers.capacity();

        cache.invalidate();

        assert!(!cache.matches(key));
        assert_eq!(cache.recipients.capacity(), recipient_capacity);
        assert_eq!(cache.frame_pointers.capacity(), frame_capacity);
    }

    #[test]
    fn client_or_resource_mutation_requires_a_new_generation() {
        let key = LockedRelativeRecipientCacheKey {
            resource_generation: 21,
            constraint_generation: 34,
            surface_id: 55,
            source_pointer_id: 89,
        };
        let cache = LockedRelativeRecipientCache {
            key: Some(key),
            ..Default::default()
        };

        assert!(!cache.matches(LockedRelativeRecipientCacheKey {
            resource_generation: key.resource_generation + 1,
            ..key
        }));
    }
}
