use super::*;

#[derive(Debug, Clone)]
pub(crate) struct NativeInputState {
    pub(crate) output_width: u32,
    pub(crate) output_height: u32,
    pub(crate) cursor_x: f64,
    pub(crate) cursor_y: f64,
    pub(crate) keyboard_shortcuts_inhibited: bool,
    keyboard_shortcut_inhibition_generation: u64,
    pub(crate) pointer_constraint: NativePointerConstraintState,
    debug_input_epoch: Option<u64>,
    debug_constraint_id: Option<PointerConstraintBackendId>,
    pub(crate) binding_manager: AstreaBindingManager,
    pub(crate) cursor_visible: bool,
    pub(crate) forwarded_control_keys: Vec<u16>,
    pub(crate) forwarded_client_keys: Vec<u16>,
    pub(crate) pressed_deferred_modifier_keys: Vec<u16>,
    pub(crate) forwarded_deferred_modifier_keys: Vec<u16>,
    pub(crate) suppressed_vt_switch_keys: Vec<u16>,
    keyboard_sources: KeyboardSourceLedger,
    keyboard_symbolic_presses: KeyboardSymbolicPressLedger,
    keyboard_repeat: KeyboardRepeatState,
    #[cfg(test)]
    pub(crate) source_removal_modifier_release_routes: Vec<(u16, ModifierMask)>,
    pressed_pointer_buttons: Vec<u32>,
}

impl NativeInputState {
    #[cfg(test)]
    pub(crate) fn new(output_width: u32, output_height: u32) -> Self {
        Self::new_with_repeat_config(output_width, output_height, KeyboardRepeatConfig::default())
    }

    pub(crate) fn new_with_repeat_config(
        output_width: u32,
        output_height: u32,
        repeat_config: KeyboardRepeatConfig,
    ) -> Self {
        Self {
            output_width: output_width.max(1),
            output_height: output_height.max(1),
            cursor_x: f64::from(output_width.max(1)) / 2.0,
            cursor_y: f64::from(output_height.max(1)) / 2.0,
            keyboard_shortcuts_inhibited: false,
            keyboard_shortcut_inhibition_generation: 0,
            pointer_constraint: NativePointerConstraintState::None,
            debug_input_epoch: None,
            debug_constraint_id: None,
            binding_manager: AstreaBindingManager::default(),
            cursor_visible: true,
            forwarded_control_keys: Vec::new(),
            forwarded_client_keys: Vec::new(),
            pressed_deferred_modifier_keys: Vec::new(),
            forwarded_deferred_modifier_keys: Vec::new(),
            suppressed_vt_switch_keys: Vec::new(),
            keyboard_sources: KeyboardSourceLedger::default(),
            keyboard_symbolic_presses: KeyboardSymbolicPressLedger::default(),
            keyboard_repeat: KeyboardRepeatState::configured(repeat_config),
            #[cfg(test)]
            source_removal_modifier_release_routes: Vec::new(),
            pressed_pointer_buttons: Vec::new(),
        }
    }

    pub(crate) fn reconfigure_output_bounds(
        &mut self,
        output_width: u32,
        output_height: u32,
    ) -> NativeInputEffect {
        self.output_width = output_width.max(1);
        self.output_height = output_height.max(1);
        match &mut self.pointer_constraint {
            NativePointerConstraintState::None => {}
            NativePointerConstraintState::Locked { anchor } => {
                anchor.x = anchor.x.clamp(0.0, f64::from(self.output_width - 1));
                anchor.y = anchor.y.clamp(0.0, f64::from(self.output_height - 1));
            }
            NativePointerConstraintState::Confined { region } => {
                *region = region.clipped_to_output(self.output_width, self.output_height);
            }
        }
        let position = self
            .pointer_constraint
            .constrain_position(CompositorOutputPosition {
                x: self.cursor_x.clamp(0.0, f64::from(self.output_width - 1)),
                y: self.cursor_y.clamp(0.0, f64::from(self.output_height - 1)),
            });
        self.cursor_x = position.x;
        self.cursor_y = position.y;
        let mut effect = NativeInputEffect::default();
        effect.mark_cursor_moved(self.cursor_x, self.cursor_y);
        effect
    }

    pub(crate) fn reconcile_keyboard_shortcut_inhibition(
        &mut self,
        snapshot: KeyboardShortcutInhibitionSnapshot,
    ) -> NativeInputEffect {
        if snapshot.generation == self.keyboard_shortcut_inhibition_generation {
            return NativeInputEffect::default();
        }

        self.keyboard_shortcut_inhibition_generation = snapshot.generation;
        if self.keyboard_shortcuts_inhibited == snapshot.effective {
            return NativeInputEffect::default();
        }

        let was_inhibited = self.keyboard_shortcuts_inhibited;
        self.keyboard_shortcuts_inhibited = snapshot.effective;
        self.cancel_keyboard_repeat_if_shortcut_inhibited(snapshot);
        let mut effect = NativeInputEffect::default();
        if !was_inhibited && snapshot.effective {
            self.binding_manager
                .cancel_shortcut_sequences_for_inhibition();
            self.replay_deferred_modifiers(&mut effect);
        }
        effect
    }

    pub(crate) fn cancel_keyboard_repeat_if_shortcut_inhibited(
        &mut self,
        snapshot: KeyboardShortcutInhibitionSnapshot,
    ) {
        self.keyboard_repeat
            .cancel_if_respected_inhibition(snapshot.effective);
    }

    pub(crate) fn cursor_position(&self) -> (i32, i32) {
        (self.cursor_x.round() as i32, self.cursor_y.round() as i32)
    }

    pub(crate) fn cursor_position_f64(&self) -> CompositorOutputPosition {
        CompositorOutputPosition {
            x: self.cursor_x,
            y: self.cursor_y,
        }
    }

    pub(crate) fn set_native_input_epoch_debug(
        &mut self,
        epoch: Option<u64>,
        constraint_id: Option<PointerConstraintBackendId>,
    ) {
        self.debug_input_epoch = epoch;
        self.debug_constraint_id = constraint_id;
    }

    pub(crate) fn set_pointer_locked_at(&mut self, anchor: CompositorOutputPosition) {
        self.pointer_constraint = NativePointerConstraintState::Locked { anchor };
    }

    pub(crate) fn set_pointer_confined(&mut self, region: OutputRegion) {
        self.pointer_constraint = NativePointerConstraintState::Confined { region };
        let position = self
            .pointer_constraint
            .constrain_position(CompositorOutputPosition {
                x: self.cursor_x,
                y: self.cursor_y,
            });
        self.cursor_x = position.x;
        self.cursor_y = position.y;
    }

    pub(crate) fn clear_pointer_constraint(&mut self) {
        self.pointer_constraint = NativePointerConstraintState::None;
    }

    pub(crate) fn set_cursor_visible(&mut self, visible: bool) -> bool {
        if self.cursor_visible == visible {
            return false;
        }
        self.cursor_visible = visible;
        true
    }

    /// Raw backend/theme-fallback visibility. Presentation must resolve the
    /// effective cursor source before treating this as final visibility.
    pub(crate) const fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    /// The backend/theme visibility signal, named for its non-authoritative role.
    pub(crate) const fn theme_fallback_visible(&self) -> bool {
        self.cursor_visible
    }

    pub(crate) fn restore_cursor_position(
        &mut self,
        position: CompositorOutputPosition,
    ) -> NativeInputEffect {
        self.cursor_x = position.x.clamp(0.0, f64::from(self.output_width - 1));
        self.cursor_y = position.y.clamp(0.0, f64::from(self.output_height - 1));
        let mut effect = NativeInputEffect::default();
        effect.mark_cursor_moved(self.cursor_x, self.cursor_y);
        effect
    }

    pub(crate) fn desktop_visual_state(
        &self,
        cursor_mode: NativeCursorRenderMode,
        cursor_visible: bool,
    ) -> DesktopVisualState {
        match cursor_mode {
            NativeCursorRenderMode::Software if cursor_visible => {
                let (x, y) = self.cursor_position();
                DesktopVisualState::with_cursor(x, y)
            }
            NativeCursorRenderMode::Software
            | NativeCursorRenderMode::SoftwareClient
            | NativeCursorRenderMode::Hardware => DesktopVisualState::wallpaper_only(),
        }
    }

    #[cfg(test)]
    pub(crate) fn handle_hardware_input_event_at(
        &mut self,
        event: NativeHardwareInputEvent,
        now_ns: u64,
    ) -> NativeInputEffect {
        self.handle_hardware_input_event_at_with_symbolic(event, None, now_ns)
    }

    pub(crate) fn handle_hardware_input_event_at_with_symbolic(
        &mut self,
        event: NativeHardwareInputEvent,
        symbolic: Option<KeyboardSymbolicSnapshot>,
        now_ns: u64,
    ) -> NativeInputEffect {
        match event {
            NativeHardwareInputEvent::Keyboard(NativeKeyboardInputEvent::Key {
                device,
                code,
                pressed,
            }) => self.handle_keyboard_key_event(device, code, pressed, symbolic, now_ns),
            NativeHardwareInputEvent::Keyboard(NativeKeyboardInputEvent::SourceRemoved {
                device,
            }) => self.handle_keyboard_source_removed(device, now_ns),
            NativeHardwareInputEvent::PointerButton { button, pressed } => {
                self.handle_pointer_button(button, pressed)
            }
            NativeHardwareInputEvent::PointerMotion(sample) => self.handle_pointer_motion(sample),
            NativeHardwareInputEvent::PointerAxis(frame) => self.handle_pointer_axis_frame(frame),
        }
    }

    #[cfg(test)]
    pub(crate) fn handle_hardware_input_event(
        &mut self,
        event: NativeHardwareInputEvent,
    ) -> NativeInputEffect {
        self.handle_hardware_input_event_at(event, 0)
    }

    fn handle_keyboard_key_event(
        &mut self,
        device: KeyboardDeviceId,
        code: u16,
        pressed: bool,
        symbolic: Option<KeyboardSymbolicSnapshot>,
        now_ns: u64,
    ) -> NativeInputEffect {
        let modifiers_before = self.active_modifier_mask();
        let transition = if pressed {
            self.keyboard_sources.press(device, code)
        } else {
            self.keyboard_sources.release(device, code)
        };
        if transition == KeyboardAggregateTransition::None {
            if !pressed {
                self.release_suppressed_vt_switch_key(code);
            }
            return NativeInputEffect::default();
        }

        let modifiers_after = self.active_modifier_mask();
        let modifier_family_released = !pressed
            && modifier_family_for_key(code).is_some_and(|family| {
                modifiers_before.contains(family) && !modifiers_after.contains(family)
            });
        let symbolic = if pressed {
            self.keyboard_symbolic_presses
                .capture(code, symbolic.unwrap_or_default());
            symbolic
        } else {
            self.keyboard_symbolic_presses.take(code)
        };
        self.route_logical_key_event(
            code,
            pressed,
            modifier_family_released,
            None,
            symbolic,
            now_ns,
        )
    }

    fn handle_keyboard_source_removed(
        &mut self,
        device: KeyboardDeviceId,
        now_ns: u64,
    ) -> NativeInputEffect {
        let modifiers_before = self.active_modifier_mask();
        let released = self.keyboard_sources.remove_source(device);
        let modifiers_after = self.active_modifier_mask();
        let mut effect = NativeInputEffect::default();

        // Removal snapshots both sides of the whole ledger mutation. If this
        // source owned both left and right keys in a family, attach the one
        // family transition to the last released key from that family.
        let mut last_modifier_release_keys = [None; 4];
        for &code in &released {
            let Some(family) = modifier_family_for_key(code) else {
                continue;
            };
            if modifiers_before.contains(family) && !modifiers_after.contains(family) {
                if let Some(index) = modifier_family_index(family) {
                    last_modifier_release_keys[index] = Some(code);
                }
            }
        }

        for code in released {
            let modifier_family_released = modifier_family_for_key(code)
                .and_then(modifier_family_index)
                .is_some_and(|index| last_modifier_release_keys[index] == Some(code));
            #[cfg(test)]
            if modifier_family_released && let Some(family) = modifier_family_for_key(code) {
                self.source_removal_modifier_release_routes
                    .push((code, family));
            }
            let symbolic = self.keyboard_symbolic_presses.take(code);
            effect.append(self.route_logical_key_event(
                code,
                false,
                modifier_family_released,
                Some(modifiers_before),
                symbolic,
                now_ns,
            ));
        }
        effect
    }

    fn route_logical_key_event(
        &mut self,
        code: u16,
        pressed: bool,
        modifier_family_released: bool,
        modifiers_for_bindings: Option<ModifierMask>,
        symbolic: Option<KeyboardSymbolicSnapshot>,
        now_ns: u64,
    ) -> NativeInputEffect {
        let mut effect = NativeInputEffect::default();
        let current_state_action =
            effect.record_keyboard_physical_event(NativeKeyboardEvent::new(code, pressed));

        let modifiers = self.active_modifier_mask();
        self.keyboard_repeat.cancel_if_modifiers_changed(modifiers);
        if !is_keyboard_modifier(code) {
            if pressed
                || self
                    .keyboard_repeat
                    .active()
                    .is_some_and(|active| active.code == code)
            {
                self.keyboard_repeat.cancel();
            }
        }

        if !pressed && self.release_suppressed_vt_switch_key(code) {
            return effect;
        }

        if is_shift_key(code) {
            if modifier_family_released
                && let AstreaBindingMatch::Consumed { action, phase, .. } = self
                    .binding_manager
                    .handle_modifier_release(ModifierMask::SHIFT)
            {
                self.apply_binding_action(action, phase, None, &mut effect);
            }
            self.forward_client_key(code, pressed, &mut effect, Some(current_state_action));
            return effect;
        }

        if is_alt_key(code) {
            self.set_deferred_modifier_pressed(code, pressed);
            let reconciled_release = self.reconcile_forwarded_deferred_modifier_key(
                code,
                pressed,
                current_state_action,
                &mut effect,
            );
            if modifier_family_released
                && let AstreaBindingMatch::Consumed { action, phase, .. } = self
                    .binding_manager
                    .handle_modifier_release(ModifierMask::ALT)
            {
                self.apply_binding_action(action, phase, None, &mut effect);
                return effect;
            }
            if !reconciled_release && self.keyboard_shortcuts_inhibited && pressed {
                self.forward_deferred_modifier_key(code, &mut effect, Some(current_state_action));
            }
            return effect;
        }

        if is_super_key(code) {
            self.set_deferred_modifier_pressed(code, pressed);
            let reconciled_release = self.reconcile_forwarded_deferred_modifier_key(
                code,
                pressed,
                current_state_action,
                &mut effect,
            );
            if modifier_family_released
                && let AstreaBindingMatch::Consumed { action, phase, .. } = self
                    .binding_manager
                    .handle_modifier_release(ModifierMask::SUPER)
            {
                self.apply_binding_action(action, phase, None, &mut effect);
            }
            if !reconciled_release && self.keyboard_shortcuts_inhibited && pressed {
                self.forward_deferred_modifier_key(code, &mut effect, Some(current_state_action));
            }
            return effect;
        }

        if is_control_key(code) {
            if modifier_family_released
                && let AstreaBindingMatch::Consumed { action, phase, .. } = self
                    .binding_manager
                    .handle_modifier_release(ModifierMask::CTRL)
            {
                self.apply_binding_action(action, phase, None, &mut effect);
            }
            if pressed {
                if !self.forwarded_control_keys.contains(&code) {
                    self.forwarded_control_keys.push(code);
                    effect.forward_keyboard_event(
                        NativeKeyboardEvent::new(code, true),
                        Some(current_state_action),
                    );
                    effect.request_redraw();
                }
            } else if self.release_forwarded_control_key(code) {
                effect.forward_keyboard_event(
                    NativeKeyboardEvent::new(code, false),
                    Some(current_state_action),
                );
                effect.request_redraw();
            }
            return effect;
        }

        if pressed
            && self.active_modifier_mask().contains(ModifierMask::CTRL)
            && self.active_modifier_mask().contains(ModifierMask::ALT)
            && let Some(vt) = vt_number_for_function_key(code)
        {
            effect.vt_switch = Some(vt);
            self.suppress_vt_switch_key(code);
            self.clear_pressed_state_for_session_switch_with_effect(&mut effect);
            return effect;
        }

        let binding_modifiers = modifiers_for_bindings.unwrap_or(modifiers);
        match self.binding_manager.handle_keyboard_key(
            binding_modifiers,
            code,
            symbolic,
            pressed,
            false,
            self.keyboard_shortcuts_inhibited,
        ) {
            AstreaBindingMatch::Consumed {
                binding,
                action,
                phase,
                repeat,
                inhibition,
            } => {
                if pressed
                    && repeat == RepeatPolicy::Enabled
                    && let Some(binding) = binding
                {
                    self.keyboard_repeat
                        .arm(binding, code, modifiers, inhibition, now_ns);
                }
                self.apply_binding_action(action, phase, None, &mut effect);
                return effect;
            }
            AstreaBindingMatch::Pass => {}
        }

        if self.keyboard_shortcuts_inhibited {
            self.replay_deferred_modifiers(&mut effect);
            self.forward_client_key(code, pressed, &mut effect, Some(current_state_action));
            return effect;
        }

        if pressed {
            self.replay_deferred_modifiers(&mut effect);
        }
        self.forward_client_key(code, pressed, &mut effect, Some(current_state_action));
        effect
    }

    #[cfg(test)]
    pub(crate) fn handle_key_event(&mut self, code: u16, value: i32) -> NativeInputEffect {
        let device = KeyboardDeviceId::from_raw(1).expect("test keyboard id is nonzero");
        self.handle_keyboard_key_event(device, code, value != 0, None, 0)
    }

    #[cfg(test)]
    pub(crate) fn handle_key_event_at(
        &mut self,
        code: u16,
        pressed: bool,
        now_ns: u64,
    ) -> NativeInputEffect {
        let device = KeyboardDeviceId::from_raw(1).expect("test keyboard id is nonzero");
        self.handle_keyboard_key_event(device, code, pressed, None, now_ns)
    }

    #[cfg(test)]
    pub(crate) fn handle_key_event_from(
        &mut self,
        device: KeyboardDeviceId,
        code: u16,
        value: i32,
    ) -> NativeInputEffect {
        self.handle_keyboard_key_event(device, code, value != 0, None, 0)
    }

    #[cfg(test)]
    pub(crate) fn handle_key_event_from_at(
        &mut self,
        device: KeyboardDeviceId,
        code: u16,
        pressed: bool,
        now_ns: u64,
    ) -> NativeInputEffect {
        self.handle_keyboard_key_event(device, code, pressed, None, now_ns)
    }

    #[cfg(test)]
    pub(crate) fn keyboard_key_is_logically_pressed(&self, code: u16) -> bool {
        self.keyboard_sources.is_logically_pressed(code)
    }

    pub(crate) fn keyboard_repeat_deadline_ns(&self) -> Option<u64> {
        self.keyboard_repeat.deadline_ns()
    }

    pub(crate) fn keyboard_repeat_due(&self, now_ns: u64) -> bool {
        self.keyboard_repeat.due(now_ns)
    }

    pub(crate) fn keyboard_repeat_generation(&self) -> u64 {
        self.keyboard_repeat.generation()
    }

    pub(crate) fn set_system_action_capabilities(
        &mut self,
        capabilities: crate::system_action::AstreaSystemActionCapabilities,
    ) -> bool {
        let changed = self.binding_manager.set_system_capabilities(capabilities);
        if changed
            && self.keyboard_repeat.active().is_some_and(|active| {
                !self
                    .binding_manager
                    .repeat_binding_available(active.binding)
            })
        {
            self.keyboard_repeat.cancel();
        }
        changed
    }

    pub(crate) const fn system_action_capability_count(&self) -> u32 {
        self.binding_manager.system_capability_count()
    }

    pub(crate) fn update_keyboard_repeat_config(
        &mut self,
        config: KeyboardRepeatConfig,
        now_ns: u64,
    ) {
        self.keyboard_repeat.update_config(config, now_ns);
    }

    pub(crate) fn clear_keyboard_repeat(&mut self) {
        self.keyboard_repeat.cancel();
    }

    pub(crate) fn keyboard_repeat_symbolic_keycode(&self) -> Option<u16> {
        let active = self.keyboard_repeat.active()?;
        self.binding_manager
            .repeat_binding_requires_symbolic_translation(active.binding)
            .then_some(active.code)
    }

    #[cfg(test)]
    pub(crate) fn service_keyboard_repeat(&mut self, now_ns: u64) -> NativeInputEffect {
        self.service_keyboard_repeat_with_symbolic(now_ns, None)
    }

    fn service_keyboard_repeat_with_symbolic(
        &mut self,
        now_ns: u64,
        symbolic: Option<KeyboardSymbolicSnapshot>,
    ) -> NativeInputEffect {
        if !self.keyboard_repeat.due(now_ns) {
            return NativeInputEffect::default();
        }
        let Some(active) = self.keyboard_repeat.active() else {
            return NativeInputEffect::default();
        };
        if active.repeat != RepeatPolicy::Enabled
            || !self.keyboard_sources.is_logically_pressed(active.code)
            || self.active_modifier_mask() != active.physical_modifiers
            || (self.keyboard_shortcuts_inhibited && active.inhibition == InhibitionPolicy::Respect)
        {
            self.keyboard_repeat.cancel();
            return NativeInputEffect::default();
        }

        let mut effect = NativeInputEffect::default();
        match self.binding_manager.match_repeat_binding(
            active.binding,
            active.code,
            active.physical_modifiers,
            symbolic,
            self.keyboard_shortcuts_inhibited,
        ) {
            Some(AstreaBindingMatch::Consumed {
                binding: Some(binding),
                action,
                phase,
                repeat,
                inhibition,
            }) if binding == active.binding
                && repeat == RepeatPolicy::Enabled
                && inhibition == active.inhibition =>
            {
                self.apply_binding_action(action, phase, None, &mut effect);
                self.keyboard_repeat.schedule_after_fire(now_ns);
            }
            Some(AstreaBindingMatch::Consumed { .. }) | Some(AstreaBindingMatch::Pass) | None => {
                self.keyboard_repeat.cancel();
            }
        }
        effect
    }

    #[cfg(test)]
    pub(crate) fn service_keyboard_repeat_if_unchanged(
        &mut self,
        now_ns: u64,
        generation: u64,
    ) -> NativeInputEffect {
        if self.keyboard_repeat_generation() != generation {
            return NativeInputEffect::default();
        }
        self.service_keyboard_repeat_with_symbolic(now_ns, None)
    }

    pub(crate) fn service_keyboard_repeat_if_unchanged_with_symbolic(
        &mut self,
        now_ns: u64,
        generation: u64,
        symbolic: Option<KeyboardSymbolicSnapshot>,
    ) -> NativeInputEffect {
        if self.keyboard_repeat_generation() != generation {
            return NativeInputEffect::default();
        }
        self.service_keyboard_repeat_with_symbolic(now_ns, symbolic)
    }

    #[cfg(test)]
    pub(crate) fn active_keyboard_repeat(&self) -> Option<ActiveKeyboardRepeat> {
        self.keyboard_repeat.active()
    }

    pub(crate) fn handle_pointer_button(
        &mut self,
        button: u32,
        pressed: bool,
    ) -> NativeInputEffect {
        let mut effect = NativeInputEffect::default();
        self.set_pointer_button_pressed(button, pressed);

        let binding_match = self.binding_manager.handle_pointer_button(
            self.active_modifier_mask(),
            button,
            pressed,
            self.keyboard_shortcuts_inhibited,
        );
        resize_debug_log(|| {
            format!(
                "event=hardware_button button={} pressed={} modifiers={:?} binding_result={} binding_action={}",
                button,
                pressed,
                self.active_modifier_mask(),
                match &binding_match {
                    AstreaBindingMatch::Consumed { .. } => "consumed",
                    AstreaBindingMatch::Pass => "pass",
                },
                match &binding_match {
                    AstreaBindingMatch::Consumed { action, .. } => format!("{action:?}"),
                    AstreaBindingMatch::Pass => "none".to_string(),
                },
            )
        });
        let binding_consumed = matches!(binding_match, AstreaBindingMatch::Consumed { .. });
        match binding_match {
            AstreaBindingMatch::Consumed { action, phase, .. } => {
                self.apply_binding_action(action, phase, Some(button), &mut effect);
            }
            AstreaBindingMatch::Pass => {}
        }

        if !binding_consumed {
            effect
                .pointer_buttons
                .push(NativePointerButtonEvent::new_at(
                    button,
                    pressed,
                    self.cursor_x,
                    self.cursor_y,
                    self.output_width,
                    self.output_height,
                ));
            effect.request_redraw();
        }
        resize_debug_log(|| {
            format!(
                "event=effect_button button={} pressed={} forwarded_to_apply={} window_actions={:?}",
                button, pressed, !binding_consumed, effect.window_actions,
            )
        });
        effect
    }

    fn set_pointer_button_pressed(&mut self, button: u32, pressed: bool) {
        if pressed {
            if !self.pressed_pointer_buttons.contains(&button) {
                self.pressed_pointer_buttons.push(button);
            }
        } else {
            self.pressed_pointer_buttons
                .retain(|pressed_button| *pressed_button != button);
        }
    }

    pub(crate) fn is_pointer_button_pressed(&self, button: u32) -> bool {
        self.pressed_pointer_buttons.contains(&button)
    }

    pub(crate) fn pressed_pointer_buttons_snapshot(&self) -> Vec<u32> {
        self.pressed_pointer_buttons.clone()
    }

    pub(crate) fn clear_pressed_pointer_buttons(&mut self) {
        self.pressed_pointer_buttons.clear();
    }

    pub(crate) fn handle_pointer_motion(
        &mut self,
        sample: PointerMotionSample,
    ) -> NativeInputEffect {
        let mut effect = NativeInputEffect {
            pointer_motion_usec: Some(sample.timestamp_usec),
            ..NativeInputEffect::default()
        };
        let locked_at_start = self.pointer_constraint.locked();
        if let Some(relative) = sample.relative {
            effect.relative_motion = (!relative.is_zero()).then_some(relative);
            if !self.pointer_constraint.locked() {
                let proposed = CompositorOutputPosition {
                    x: (self.cursor_x + relative.dx).clamp(0.0, f64::from(self.output_width - 1)),
                    y: (self.cursor_y + relative.dy).clamp(0.0, f64::from(self.output_height - 1)),
                };
                let constrained = self.pointer_constraint.constrain_position(proposed);
                self.cursor_x = constrained.x;
                self.cursor_y = constrained.y;
            }
        }
        if let Some((x, y)) = sample.absolute
            && !self.pointer_constraint.locked()
        {
            let proposed = CompositorOutputPosition {
                x: x.clamp(0.0, f64::from(self.output_width - 1)),
                y: y.clamp(0.0, f64::from(self.output_height - 1)),
            };
            let constrained = self.pointer_constraint.constrain_position(proposed);
            self.cursor_x = constrained.x;
            self.cursor_y = constrained.y;
        }
        if !self.pointer_constraint.locked() {
            effect.pointer_motion = Some((self.cursor_x, self.cursor_y));
        }
        if !self.pointer_constraint.locked() {
            effect.mark_cursor_moved(self.cursor_x, self.cursor_y);
        }
        native_pointer_debug_log_lazy(|| {
            format!(
                "pointer.motion native epoch={:?} constraint={:?} timestamp_usec={} locked={} absolute_updated={} relative=({},{}) cursor=({},{})",
                self.debug_input_epoch,
                self.debug_constraint_id,
                sample.timestamp_usec,
                locked_at_start,
                effect.pointer_motion.is_some(),
                sample.relative.map(|relative| relative.dx).unwrap_or(0.0),
                sample.relative.map(|relative| relative.dy).unwrap_or(0.0),
                self.cursor_x,
                self.cursor_y
            )
        });
        effect
    }

    #[cfg(test)]
    pub(crate) fn handle_pointer_motion_delta(&mut self, dx: f64, dy: f64) -> NativeInputEffect {
        self.handle_pointer_motion(PointerMotionSample::relative(
            0,
            RelativeMotion::accelerated_only(dx, dy),
        ))
    }

    #[cfg(test)]
    pub(crate) fn handle_pointer_axis(
        &mut self,
        horizontal: f64,
        vertical: f64,
    ) -> NativeInputEffect {
        self.handle_pointer_axis_frame(PointerAxisFrame::unknown(0, horizontal, vertical))
    }

    pub(crate) fn handle_pointer_axis_frame(
        &mut self,
        frame: PointerAxisFrame,
    ) -> NativeInputEffect {
        let mut effect = NativeInputEffect {
            pointer_axis: Some(frame),
            ..Default::default()
        };
        effect.request_redraw();
        effect
    }

    pub(crate) fn release_forwarded_control_key(&mut self, code: u16) -> bool {
        let Some(index) = self
            .forwarded_control_keys
            .iter()
            .position(|forwarded| *forwarded == code)
        else {
            return false;
        };
        self.forwarded_control_keys.swap_remove(index);
        true
    }

    fn forward_deferred_modifier_key(
        &mut self,
        code: u16,
        effect: &mut NativeInputEffect,
        current_state_action: Option<usize>,
    ) {
        if self.forwarded_deferred_modifier_keys.contains(&code) {
            return;
        }
        self.forwarded_deferred_modifier_keys.push(code);
        let event = NativeKeyboardEvent::new(code, true);
        if current_state_action.is_some() {
            effect.forward_keyboard_event(event, current_state_action);
        } else {
            effect.forward_keyboard_event_client_only(event);
        }
        effect.request_redraw();
    }

    fn forward_client_key(
        &mut self,
        code: u16,
        pressed: bool,
        effect: &mut NativeInputEffect,
        current_state_action: Option<usize>,
    ) {
        if pressed {
            if !self.forwarded_client_keys.contains(&code) {
                self.forwarded_client_keys.push(code);
            }
            effect
                .forward_keyboard_event(NativeKeyboardEvent::new(code, true), current_state_action);
            effect.request_redraw();
            return;
        }
        let Some(index) = self
            .forwarded_client_keys
            .iter()
            .position(|forwarded| *forwarded == code)
        else {
            return;
        };
        self.forwarded_client_keys.swap_remove(index);
        effect.forward_keyboard_event(NativeKeyboardEvent::new(code, false), current_state_action);
        effect.request_redraw();
    }

    fn set_deferred_modifier_pressed(&mut self, code: u16, pressed: bool) {
        if pressed {
            if !self.pressed_deferred_modifier_keys.contains(&code) {
                self.pressed_deferred_modifier_keys.push(code);
            }
            return;
        }
        if let Some(index) = self
            .pressed_deferred_modifier_keys
            .iter()
            .position(|pressed_code| *pressed_code == code)
        {
            self.pressed_deferred_modifier_keys.swap_remove(index);
        }
    }

    fn release_forwarded_deferred_modifier_key(&mut self, code: u16) -> bool {
        let Some(index) = self
            .forwarded_deferred_modifier_keys
            .iter()
            .position(|forwarded| *forwarded == code)
        else {
            return false;
        };
        self.forwarded_deferred_modifier_keys.swap_remove(index);
        true
    }

    fn reconcile_forwarded_deferred_modifier_key(
        &mut self,
        code: u16,
        pressed: bool,
        current_state_action: usize,
        effect: &mut NativeInputEffect,
    ) -> bool {
        if pressed || !self.release_forwarded_deferred_modifier_key(code) {
            return false;
        }
        effect.forward_keyboard_event(
            NativeKeyboardEvent::new(code, false),
            Some(current_state_action),
        );
        effect.request_redraw();
        true
    }

    fn suppress_vt_switch_key(&mut self, code: u16) {
        if !self.suppressed_vt_switch_keys.contains(&code) {
            self.suppressed_vt_switch_keys.push(code);
        }
    }

    fn release_suppressed_vt_switch_key(&mut self, code: u16) -> bool {
        let Some(index) = self
            .suppressed_vt_switch_keys
            .iter()
            .position(|suppressed| *suppressed == code)
        else {
            return false;
        };
        self.suppressed_vt_switch_keys.swap_remove(index);
        true
    }

    fn replay_deferred_modifiers(&mut self, effect: &mut NativeInputEffect) {
        for code in self.pressed_deferred_modifier_keys.clone() {
            self.forward_deferred_modifier_key(code, effect, None);
        }
    }

    pub(crate) fn active_modifier_mask(&self) -> ModifierMask {
        let mut mask = ModifierMask::EMPTY;
        if self.keyboard_sources.is_logically_pressed(KEY_LEFTALT)
            || self.keyboard_sources.is_logically_pressed(KEY_RIGHTALT)
        {
            mask = mask | ModifierMask::ALT;
        }
        if self.keyboard_sources.is_logically_pressed(KEY_LEFTSHIFT)
            || self.keyboard_sources.is_logically_pressed(KEY_RIGHTSHIFT)
        {
            mask = mask | ModifierMask::SHIFT;
        }
        if self.keyboard_sources.is_logically_pressed(KEY_LEFTMETA)
            || self.keyboard_sources.is_logically_pressed(KEY_RIGHTMETA)
        {
            mask = mask | ModifierMask::SUPER;
        }
        if self.keyboard_sources.is_logically_pressed(KEY_LEFTCTRL)
            || self.keyboard_sources.is_logically_pressed(KEY_RIGHTCTRL)
        {
            mask = mask | ModifierMask::CTRL;
        }
        mask
    }

    #[cfg(test)]
    pub(crate) fn forwarded_key_ledger_is_empty(&self) -> bool {
        self.forwarded_control_keys.is_empty()
            && self.forwarded_client_keys.is_empty()
            && self.forwarded_deferred_modifier_keys.is_empty()
    }

    fn apply_binding_action(
        &mut self,
        action: BindingActionId,
        phase: AstreaShortcutPhase,
        trigger_button: Option<u32>,
        effect: &mut NativeInputEffect,
    ) {
        let action_id = action;
        let Some(action) = self.binding_manager.action_catalog().action(action_id) else {
            return;
        };
        match action {
            BindingActionDefinition::ExitCompositor => {
                effect.exit_requested = true;
            }
            BindingActionDefinition::CloseActiveWindow => {
                effect
                    .window_actions
                    .push(NativeWindowAction::CloseActiveWindow);
                effect.request_visual_redraw();
            }
            BindingActionDefinition::ToggleFullscreen => {
                effect
                    .window_actions
                    .push(NativeWindowAction::ToggleFullscreen);
                effect.request_visual_redraw();
            }
            BindingActionDefinition::ToggleFocusedWindowLayout => {
                effect
                    .window_actions
                    .push(NativeWindowAction::ToggleFocusedWindowLayout);
                effect.request_visual_redraw();
            }
            BindingActionDefinition::SwitchWorkspace(workspace) => {
                effect
                    .window_actions
                    .push(NativeWindowAction::SwitchWorkspace(*workspace));
                effect.request_visual_redraw();
            }
            BindingActionDefinition::MoveFocusedWindowToWorkspace(workspace) => {
                effect
                    .window_actions
                    .push(NativeWindowAction::MoveFocusedWindowToWorkspace(*workspace));
                effect.request_visual_redraw();
            }
            BindingActionDefinition::ToggleDefaultSpecialWorkspace => {
                effect
                    .window_actions
                    .push(NativeWindowAction::ToggleDefaultSpecialWorkspace);
                effect.request_visual_redraw();
            }
            BindingActionDefinition::MoveFocusedWindowToOrFromSpecialWorkspace => {
                effect
                    .window_actions
                    .push(NativeWindowAction::MoveFocusedWindowToOrFromSpecialWorkspace);
                effect.request_visual_redraw();
            }
            BindingActionDefinition::LaunchCommand(_)
            | BindingActionDefinition::LaunchSessionCommand { .. }
            | BindingActionDefinition::EmitShortcut { .. }
            | BindingActionDefinition::SystemAction(_) => {
                effect
                    .binding_action_invocations
                    .push(BindingActionInvocation {
                        action: action_id,
                        phase,
                    });
            }
            BindingActionDefinition::BeginMove => {
                effect.window_actions.push(NativeWindowAction::BeginMove {
                    x: self.cursor_x,
                    y: self.cursor_y,
                    trigger_button,
                });
                effect.request_visual_redraw();
            }
            BindingActionDefinition::BeginResize => {
                effect.window_actions.push(NativeWindowAction::BeginResize {
                    x: self.cursor_x,
                    y: self.cursor_y,
                    trigger_button,
                });
                effect.request_visual_redraw();
            }
        }
    }

    fn clear_pressed_state_for_session_switch_with_effect(
        &mut self,
        effect: &mut NativeInputEffect,
    ) {
        for code in self.keyboard_sources.logical_pressed_keys() {
            let _ = effect.record_keyboard_physical_event(NativeKeyboardEvent::new(code, false));
        }
        self.clear_pressed_state_for_session_switch();
    }

    pub(crate) fn clear_pressed_state_for_session_switch(&mut self) {
        self.clear_keyboard_repeat();
        self.keyboard_sources.clear();
        self.keyboard_symbolic_presses.clear();
        self.forwarded_control_keys.clear();
        self.forwarded_client_keys.clear();
        self.pressed_deferred_modifier_keys.clear();
        self.forwarded_deferred_modifier_keys.clear();
        self.suppressed_vt_switch_keys.clear();
        self.clear_pressed_pointer_buttons();
        self.pointer_constraint = NativePointerConstraintState::None;
    }
}

pub(crate) fn is_shift_key(code: u16) -> bool {
    matches!(code, KEY_LEFTSHIFT | KEY_RIGHTSHIFT)
}

pub(crate) fn is_alt_key(code: u16) -> bool {
    matches!(code, KEY_LEFTALT | KEY_RIGHTALT)
}

pub(crate) fn is_super_key(code: u16) -> bool {
    matches!(code, KEY_LEFTMETA | KEY_RIGHTMETA)
}

pub(crate) fn is_control_key(code: u16) -> bool {
    matches!(code, KEY_LEFTCTRL | KEY_RIGHTCTRL)
}

fn is_keyboard_modifier(code: u16) -> bool {
    is_shift_key(code) || is_control_key(code) || is_alt_key(code) || is_super_key(code)
}

fn modifier_family_for_key(code: u16) -> Option<ModifierMask> {
    if is_shift_key(code) {
        Some(ModifierMask::SHIFT)
    } else if is_control_key(code) {
        Some(ModifierMask::CTRL)
    } else if is_alt_key(code) {
        Some(ModifierMask::ALT)
    } else if is_super_key(code) {
        Some(ModifierMask::SUPER)
    } else {
        None
    }
}

fn modifier_family_index(family: ModifierMask) -> Option<usize> {
    if family == ModifierMask::ALT {
        Some(0)
    } else if family == ModifierMask::CTRL {
        Some(1)
    } else if family == ModifierMask::SHIFT {
        Some(2)
    } else if family == ModifierMask::SUPER {
        Some(3)
    } else {
        None
    }
}

pub(crate) fn is_pointer_button(code: u16) -> bool {
    matches!(code, BTN_LEFT | BTN_RIGHT | BTN_MIDDLE)
}

pub(crate) const fn vt_number_for_function_key(code: u16) -> Option<u8> {
    match code {
        KEY_F1 => Some(1),
        KEY_F2 => Some(2),
        KEY_F3 => Some(3),
        KEY_F4 => Some(4),
        KEY_F5 => Some(5),
        KEY_F6 => Some(6),
        KEY_F7 => Some(7),
        KEY_F8 => Some(8),
        KEY_F9 => Some(9),
        KEY_F10 => Some(10),
        KEY_F11 => Some(11),
        KEY_F12 => Some(12),
        _ => None,
    }
}
