use super::*;

#[derive(Debug)]
pub(crate) struct NativeInputState {
    pub(crate) output_width: u32,
    pub(crate) output_height: u32,
    pub(crate) cursor_x: f64,
    pub(crate) cursor_y: f64,
    pub(crate) alt_pressed: bool,
    pub(crate) ctrl_pressed: bool,
    pub(crate) super_pressed: bool,
    pub(crate) shift_pressed: bool,
    pub(crate) keyboard_shortcuts_inhibited: bool,
    pub(crate) pointer_constraint: NativePointerConstraintState,
    pub(crate) binding_manager: AstreaBindingManager,
    pub(crate) cursor_visible: bool,
    pub(crate) forwarded_control_keys: Vec<u16>,
    pub(crate) pressed_deferred_modifier_keys: Vec<u16>,
    pub(crate) forwarded_deferred_modifier_keys: Vec<u16>,
    pub(crate) suppressed_vt_switch_keys: Vec<u16>,
}

impl NativeInputState {
    pub(crate) fn new(output_width: u32, output_height: u32) -> Self {
        Self {
            output_width: output_width.max(1),
            output_height: output_height.max(1),
            cursor_x: f64::from(output_width.max(1)) / 2.0,
            cursor_y: f64::from(output_height.max(1)) / 2.0,
            alt_pressed: false,
            ctrl_pressed: false,
            super_pressed: false,
            shift_pressed: false,
            keyboard_shortcuts_inhibited: false,
            pointer_constraint: NativePointerConstraintState::None,
            binding_manager: AstreaBindingManager::default(),
            cursor_visible: true,
            forwarded_control_keys: Vec::new(),
            pressed_deferred_modifier_keys: Vec::new(),
            forwarded_deferred_modifier_keys: Vec::new(),
            suppressed_vt_switch_keys: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn set_keyboard_shortcuts_inhibited(&mut self, inhibited: bool) {
        self.keyboard_shortcuts_inhibited = inhibited;
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
    ) -> DesktopVisualState {
        match cursor_mode {
            NativeCursorRenderMode::Software if self.cursor_visible => {
                let (x, y) = self.cursor_position();
                DesktopVisualState::with_cursor(x, y)
            }
            NativeCursorRenderMode::Software | NativeCursorRenderMode::Hardware => {
                DesktopVisualState::wallpaper_only()
            }
        }
    }

    pub(crate) fn handle_hardware_input_event(
        &mut self,
        event: NativeHardwareInputEvent,
    ) -> NativeInputEffect {
        match event {
            NativeHardwareInputEvent::Key { code, value } => self.handle_key_event(code, value),
            NativeHardwareInputEvent::PointerButton { button, pressed } => {
                self.handle_pointer_button(button, pressed)
            }
            NativeHardwareInputEvent::PointerMotion(sample) => self.handle_pointer_motion(sample),
            NativeHardwareInputEvent::PointerAxis {
                horizontal,
                vertical,
            } => self.handle_pointer_axis(horizontal, vertical),
        }
    }

    pub(crate) fn handle_key_event(&mut self, code: u16, value: i32) -> NativeInputEffect {
        let pressed = value != 0;
        let repeated = value == 2;
        let mut effect = NativeInputEffect::default();

        if !pressed && self.release_suppressed_vt_switch_key(code) {
            return effect;
        }

        if is_shift_key(code) {
            self.shift_pressed = pressed;
            if !pressed
                && let AstreaBindingMatch::Consumed(action) = self
                    .binding_manager
                    .handle_modifier_release(ModifierMask::SHIFT)
            {
                self.apply_binding_action(action, repeated, None, &mut effect);
            }
            if !repeated {
                effect
                    .keyboard_events
                    .push(NativeKeyboardEvent::new(code, pressed));
                effect.request_redraw();
            }
            return effect;
        }

        if is_alt_key(code) {
            self.alt_pressed = pressed;
            self.set_deferred_modifier_pressed(code, pressed);
            if !pressed
                && let AstreaBindingMatch::Consumed(action) = self
                    .binding_manager
                    .handle_modifier_release(ModifierMask::ALT)
            {
                self.apply_binding_action(action, repeated, None, &mut effect);
                return effect;
            }
            if !repeated && self.release_forwarded_deferred_modifier_key(code) {
                effect
                    .keyboard_events
                    .push(NativeKeyboardEvent::new(code, pressed));
                effect.request_redraw();
            } else if self.keyboard_shortcuts_inhibited && pressed && !repeated {
                self.forward_deferred_modifier_key(code, &mut effect);
            }
            return effect;
        }

        if is_super_key(code) {
            self.super_pressed = pressed;
            self.set_deferred_modifier_pressed(code, pressed);
            if !pressed
                && let AstreaBindingMatch::Consumed(action) = self
                    .binding_manager
                    .handle_modifier_release(ModifierMask::SUPER)
            {
                self.apply_binding_action(action, repeated, None, &mut effect);
            }
            if !repeated && self.release_forwarded_deferred_modifier_key(code) {
                effect
                    .keyboard_events
                    .push(NativeKeyboardEvent::new(code, pressed));
                effect.request_redraw();
            } else if self.keyboard_shortcuts_inhibited && pressed && !repeated {
                self.forward_deferred_modifier_key(code, &mut effect);
            }
            return effect;
        }

        if is_control_key(code) {
            self.ctrl_pressed = pressed;
            if !pressed
                && let AstreaBindingMatch::Consumed(action) = self
                    .binding_manager
                    .handle_modifier_release(ModifierMask::CTRL)
            {
                self.apply_binding_action(action, repeated, None, &mut effect);
            }
            if pressed {
                if !self.forwarded_control_keys.contains(&code) {
                    self.forwarded_control_keys.push(code);
                    effect
                        .keyboard_events
                        .push(NativeKeyboardEvent::new(code, true));
                    effect.request_redraw();
                }
            } else if self.release_forwarded_control_key(code) {
                effect
                    .keyboard_events
                    .push(NativeKeyboardEvent::new(code, false));
                effect.request_redraw();
            }
            return effect;
        }

        if pressed
            && !repeated
            && self.ctrl_pressed
            && self.alt_pressed
            && let Some(vt) = vt_number_for_function_key(code)
        {
            effect.vt_switch = Some(vt);
            self.suppress_vt_switch_key(code);
            self.clear_pressed_state_for_session_switch();
            return effect;
        }

        match self.binding_manager.handle_key(
            self.active_modifier_mask(),
            code,
            pressed,
            repeated,
            self.keyboard_shortcuts_inhibited,
        ) {
            AstreaBindingMatch::Consumed(action) => {
                self.apply_binding_action(action, repeated, None, &mut effect);
                return effect;
            }
            AstreaBindingMatch::Pass => {}
        }

        if pressed
            && !repeated
            && self.super_pressed
            && code == KEY_SPACE
            && let Some(command) = external_spotlight_command()
        {
            effect.launch_command = Some(command);
            effect.launch_source = Some(NativeLaunchSource::Spotlight);
            return effect;
        }

        if pressed
            && !repeated
            && self.alt_pressed
            && code == KEY_TAB
            && let Some(command) = external_alt_tab_command()
        {
            effect.launch_command = Some(command);
            effect.launch_source = Some(NativeLaunchSource::AltTab);
            return effect;
        }

        if self.keyboard_shortcuts_inhibited {
            if !repeated {
                self.replay_deferred_modifiers(&mut effect);
                effect
                    .keyboard_events
                    .push(NativeKeyboardEvent::new(code, pressed));
                effect.request_redraw();
            }
            return effect;
        }

        if !repeated {
            if pressed {
                self.replay_deferred_modifiers(&mut effect);
            }
            effect
                .keyboard_events
                .push(NativeKeyboardEvent::new(code, pressed));
            effect.request_redraw();
        }
        effect
    }

    pub(crate) fn handle_pointer_button(
        &mut self,
        button: u32,
        pressed: bool,
    ) -> NativeInputEffect {
        let mut effect = NativeInputEffect::default();

        match self.binding_manager.handle_pointer_button(
            self.active_modifier_mask(),
            button,
            pressed,
            self.keyboard_shortcuts_inhibited,
        ) {
            AstreaBindingMatch::Consumed(action) => {
                self.apply_binding_action(action, false, Some(button), &mut effect);
                return effect;
            }
            AstreaBindingMatch::Pass => {}
        }

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
        effect
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
        native_pointer_debug_log(format!(
            "pointer.motion native locked={} absolute_updated={} relative=({},{}) cursor=({},{})",
            locked_at_start,
            effect.pointer_motion.is_some(),
            sample.relative.map(|relative| relative.dx).unwrap_or(0.0),
            sample.relative.map(|relative| relative.dy).unwrap_or(0.0),
            self.cursor_x,
            self.cursor_y
        ));
        effect
    }

    #[cfg(test)]
    pub(crate) fn handle_pointer_motion_delta(&mut self, dx: f64, dy: f64) -> NativeInputEffect {
        self.handle_pointer_motion(PointerMotionSample::relative(
            0,
            RelativeMotion::accelerated_only(dx, dy),
        ))
    }

    pub(crate) fn handle_pointer_axis(
        &mut self,
        horizontal: f64,
        vertical: f64,
    ) -> NativeInputEffect {
        let mut effect = NativeInputEffect {
            pointer_axis: Some((horizontal, vertical)),
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

    fn forward_deferred_modifier_key(&mut self, code: u16, effect: &mut NativeInputEffect) {
        if self.forwarded_deferred_modifier_keys.contains(&code) {
            return;
        }
        self.forwarded_deferred_modifier_keys.push(code);
        effect
            .keyboard_events
            .push(NativeKeyboardEvent::new(code, true));
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
            self.forward_deferred_modifier_key(code, effect);
        }
    }

    pub(crate) fn active_modifier_mask(&self) -> ModifierMask {
        let mut mask = ModifierMask::EMPTY;
        if self.alt_pressed {
            mask = mask | ModifierMask::ALT;
        }
        if self.shift_pressed {
            mask = mask | ModifierMask::SHIFT;
        }
        if self.super_pressed {
            mask = mask | ModifierMask::SUPER;
        }
        if self.ctrl_pressed {
            mask = mask | ModifierMask::CTRL;
        }
        mask
    }

    fn apply_binding_action(
        &mut self,
        action: BindingAction,
        repeated: bool,
        trigger_button: Option<u32>,
        effect: &mut NativeInputEffect,
    ) {
        match action {
            BindingAction::ExitCompositor => {
                effect.exit_requested = true;
            }
            BindingAction::CloseActiveWindow => {
                effect
                    .window_actions
                    .push(NativeWindowAction::CloseActiveWindow);
                effect.request_visual_redraw();
            }
            BindingAction::ToggleFullscreen => {
                effect
                    .window_actions
                    .push(NativeWindowAction::ToggleFullscreen);
                effect.request_visual_redraw();
            }
            BindingAction::LaunchCommand(command) => {
                effect.launch_command = Some(command);
                effect.launch_source = Some(NativeLaunchSource::BindingApplication);
            }
            BindingAction::LaunchSessionCommand(index) => {
                if let Some(command) = external_session_switch_command(index) {
                    effect.launch_command = Some(command);
                    effect.launch_source = Some(NativeLaunchSource::BindingSessionCommand);
                }
            }
            BindingAction::BeginMove => {
                effect.window_actions.push(NativeWindowAction::BeginMove {
                    x: self.cursor_x,
                    y: self.cursor_y,
                    trigger_button,
                });
                effect.request_visual_redraw();
            }
            BindingAction::BeginResize => {
                effect.window_actions.push(NativeWindowAction::BeginResize {
                    x: self.cursor_x,
                    y: self.cursor_y,
                    trigger_button,
                });
                effect.request_visual_redraw();
            }
            BindingAction::EmitShortcut { namespace, name } => {
                effect.shortcut_events.push(AstreaShortcutEvent {
                    namespace,
                    name,
                    repeated,
                });
            }
        }
    }

    pub(crate) fn clear_pressed_state_for_session_switch(&mut self) {
        self.alt_pressed = false;
        self.ctrl_pressed = false;
        self.super_pressed = false;
        self.shift_pressed = false;
        self.forwarded_control_keys.clear();
        self.pressed_deferred_modifier_keys.clear();
        self.forwarded_deferred_modifier_keys.clear();
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
