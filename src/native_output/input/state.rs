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
    pub(crate) window_interaction_active: bool,
    pub(crate) keyboard_shortcuts_inhibited: bool,
    pub(crate) pointer_constraint: NativePointerConstraintState,
    pub(crate) cursor_visible: bool,
    pub(crate) forwarded_control_keys: Vec<u16>,
    pub(crate) suppressed_window_shortcut_keys: Vec<u16>,
    pub(crate) spotlight: SpotlightModel,
    pub(crate) shell_generation: u64,
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
            window_interaction_active: false,
            keyboard_shortcuts_inhibited: false,
            pointer_constraint: NativePointerConstraintState::None,
            cursor_visible: true,
            forwarded_control_keys: Vec::new(),
            suppressed_window_shortcut_keys: Vec::new(),
            spotlight: SpotlightModel::default(),
            shell_generation: 0,
        }
    }

    pub(crate) fn spotlight_visible(&self) -> bool {
        self.spotlight.is_visible()
    }

    #[cfg(test)]
    pub(crate) fn spotlight_query(&self) -> &str {
        self.spotlight.query()
    }

    pub(crate) const fn shell_generation(&self) -> u64 {
        self.shell_generation
    }

    pub(crate) const fn spotlight(&self) -> &SpotlightModel {
        &self.spotlight
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

        if is_shift_key(code) {
            self.shift_pressed = pressed;
            if !self.spotlight_visible() && !repeated {
                effect
                    .keyboard_events
                    .push(NativeKeyboardEvent::new(code, pressed));
                effect.request_redraw();
            }
            return effect;
        }

        if is_alt_key(code) {
            self.alt_pressed = pressed;
            if self.keyboard_shortcuts_inhibited && !repeated {
                effect
                    .keyboard_events
                    .push(NativeKeyboardEvent::new(code, pressed));
                effect.request_redraw();
            }
            if !pressed && self.window_interaction_active {
                self.window_interaction_active = false;
                effect
                    .window_actions
                    .push(NativeWindowAction::EndInteraction);
                effect.request_visual_redraw();
            }
            return effect;
        }

        if is_super_key(code) {
            self.super_pressed = pressed;
            if self.keyboard_shortcuts_inhibited && !repeated {
                effect
                    .keyboard_events
                    .push(NativeKeyboardEvent::new(code, pressed));
                effect.request_redraw();
            }
            return effect;
        }

        if is_control_key(code) {
            self.ctrl_pressed = pressed;
            if self.spotlight_visible() {
                return effect;
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

        if pressed && !repeated && self.alt_pressed && code == KEY_P {
            effect.exit_requested = true;
            return effect;
        }

        if self.keyboard_shortcuts_inhibited && !self.spotlight_visible() {
            if !repeated {
                effect
                    .keyboard_events
                    .push(NativeKeyboardEvent::new(code, pressed));
                effect.request_redraw();
            }
            return effect;
        }

        if !pressed && self.release_suppressed_window_shortcut_key(code) {
            return effect;
        }

        if pressed && !repeated && self.is_spotlight_toggle_key(code) {
            effect
                .keyboard_events
                .extend(self.release_forwarded_control_modifiers());
            self.spotlight.toggle();
            self.bump_shell_generation();
            effect.request_visual_redraw();
            return effect;
        }

        if self.spotlight_visible() {
            if pressed {
                self.handle_spotlight_key(code, &mut effect);
            }
            return effect;
        }

        if let Some(shortcut) = native_window_management_shortcut(self.alt_pressed, code) {
            if pressed && !repeated && self.suppress_window_shortcut_key(code) {
                effect.window_actions.push(shortcut.into_action());
                effect.request_visual_redraw();
            }
            return effect;
        }

        if self.alt_pressed {
            return effect;
        }

        if !repeated {
            effect
                .keyboard_events
                .push(NativeKeyboardEvent::new(code, pressed));
            effect.request_redraw();
        }
        effect
    }

    pub(crate) fn handle_spotlight_key(&mut self, code: u16, effect: &mut NativeInputEffect) {
        match code {
            KEY_ESC => {
                self.spotlight.hide();
                self.bump_shell_generation();
                effect.request_visual_redraw();
            }
            KEY_BACKSPACE => {
                if self.spotlight.backspace() {
                    self.bump_shell_generation();
                    effect.request_visual_redraw();
                }
            }
            KEY_DOWN => {
                if self.spotlight.select_next() {
                    self.bump_shell_generation();
                    effect.request_visual_redraw();
                }
            }
            KEY_UP => {
                if self.spotlight.select_previous() {
                    self.bump_shell_generation();
                    effect.request_visual_redraw();
                }
            }
            KEY_ENTER => {
                effect.launch_command = self.spotlight.selected_launch_command();
                self.spotlight.hide();
                self.bump_shell_generation();
                effect.request_visual_redraw();
            }
            _ => {
                if let Some(text) = evdev_key_to_text(code, self.shift_pressed) {
                    self.spotlight.push_text(text);
                    self.bump_shell_generation();
                    effect.request_visual_redraw();
                }
            }
        }
    }

    pub(crate) fn handle_pointer_button(
        &mut self,
        button: u32,
        pressed: bool,
    ) -> NativeInputEffect {
        if self.spotlight_visible() {
            return NativeInputEffect::default();
        }
        let mut effect = NativeInputEffect::default();
        if self.window_interaction_active && !pressed {
            self.window_interaction_active = false;
            effect
                .window_actions
                .push(NativeWindowAction::EndInteraction);
            effect.request_visual_redraw();
            return effect;
        }

        if pressed
            && let Some(action) =
                native_window_drag_action(self.alt_pressed, button, self.cursor_x, self.cursor_y)
        {
            self.window_interaction_active = true;
            effect.window_actions.push(action);
            effect.request_visual_redraw();
            return effect;
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
        let shell_captures_pointer = self.spotlight_visible();
        if let Some(relative) = sample.relative {
            effect.relative_motion =
                (!shell_captures_pointer && !relative.is_zero()).then_some(relative);
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
        if !self.pointer_constraint.locked() && !self.spotlight_visible() {
            if self.window_interaction_active {
                effect
                    .window_actions
                    .push(NativeWindowAction::UpdateInteraction {
                        x: self.cursor_x,
                        y: self.cursor_y,
                    });
                effect.request_visual_redraw();
            } else {
                effect.pointer_motion = Some((self.cursor_x, self.cursor_y));
            }
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
        let mut effect = NativeInputEffect::default();
        if !self.spotlight_visible() {
            effect.pointer_axis = Some((horizontal, vertical));
            effect.request_redraw();
        }
        effect
    }

    pub(crate) fn is_spotlight_toggle_key(&self, code: u16) -> bool {
        code == KEY_SPACE && (self.super_pressed || self.ctrl_pressed)
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

    pub(crate) fn release_forwarded_control_modifiers(&mut self) -> Vec<NativeKeyboardEvent> {
        self.forwarded_control_keys
            .drain(..)
            .map(|key| NativeKeyboardEvent::new(key, false))
            .collect()
    }

    pub(crate) fn suppress_window_shortcut_key(&mut self, code: u16) -> bool {
        if self.suppressed_window_shortcut_keys.contains(&code) {
            return false;
        }
        self.suppressed_window_shortcut_keys.push(code);
        true
    }

    pub(crate) fn release_suppressed_window_shortcut_key(&mut self, code: u16) -> bool {
        let Some(index) = self
            .suppressed_window_shortcut_keys
            .iter()
            .position(|suppressed| *suppressed == code)
        else {
            return false;
        };

        self.suppressed_window_shortcut_keys.swap_remove(index);
        true
    }

    pub(crate) fn bump_shell_generation(&mut self) {
        self.shell_generation = self.shell_generation.wrapping_add(1);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeWindowManagementShortcut {
    Minimize,
    RestoreMinimized,
    ToggleMaximize,
    ToggleFullscreen,
}

impl NativeWindowManagementShortcut {
    pub(crate) const fn into_action(self) -> NativeWindowAction {
        match self {
            Self::Minimize => NativeWindowAction::Minimize,
            Self::RestoreMinimized => NativeWindowAction::RestoreMinimized,
            Self::ToggleMaximize => NativeWindowAction::ToggleMaximize,
            Self::ToggleFullscreen => NativeWindowAction::ToggleFullscreen,
        }
    }
}

pub(crate) fn native_window_management_shortcut(
    alt_pressed: bool,
    code: u16,
) -> Option<NativeWindowManagementShortcut> {
    if !alt_pressed {
        return None;
    }

    match code {
        KEY_M => Some(NativeWindowManagementShortcut::Minimize),
        KEY_R => Some(NativeWindowManagementShortcut::RestoreMinimized),
        KEY_F => Some(NativeWindowManagementShortcut::ToggleMaximize),
        KEY_ENTER | KEY_F11 => Some(NativeWindowManagementShortcut::ToggleFullscreen),
        _ => None,
    }
}
