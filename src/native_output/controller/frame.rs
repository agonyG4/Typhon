pub(crate) const ABS_AXIS_COUNT: usize = 64;
pub(crate) const KEY_CODE_COUNT: usize = 0x300;
const KEY_WORDS: usize = KEY_CODE_COUNT.div_ceil(u64::BITS as usize);
const HAT0_X: u16 = 0x10;
const HAT0_Y: u16 = 0x11;
const BTN_DPAD_UP: u16 = 0x220;
const BTN_DPAD_RIGHT: u16 = 0x223;
const ACTIVITY_ENTER: f32 = 0.18;
const ACTIVITY_EXIT: f32 = 0.12;

pub(crate) const ABS_X: u16 = 0x00;
pub(crate) const ABS_Y: u16 = 0x01;
pub(crate) const ABS_Z: u16 = 0x02;
pub(crate) const ABS_RX: u16 = 0x03;
pub(crate) const ABS_RY: u16 = 0x04;
pub(crate) const ABS_RZ: u16 = 0x05;
#[cfg(test)]
pub(crate) const ABS_HAT0X: u16 = HAT0_X;
#[cfg(test)]
pub(crate) const ABS_HAT0Y: u16 = HAT0_Y;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AxisRange {
    minimum: i32,
    maximum: i32,
    initial_value: i32,
    supported: bool,
}

impl AxisRange {
    pub(crate) const fn new(minimum: i32, maximum: i32) -> Self {
        Self {
            minimum,
            maximum,
            initial_value: minimum.saturating_add(maximum.saturating_sub(minimum) / 2),
            supported: maximum > minimum,
        }
    }

    pub(crate) const fn with_initial_value(mut self, value: i32) -> Self {
        self.initial_value = value;
        self
    }

    fn normalize_signed(self, value: i32) -> f32 {
        if !self.supported {
            return 0.0;
        }
        let midpoint =
            f64::from(self.minimum) + (f64::from(self.maximum) - f64::from(self.minimum)) / 2.0;
        let value = f64::from(value);
        let normalized = if value < midpoint {
            (value - midpoint) / (midpoint - f64::from(self.minimum))
        } else {
            (value - midpoint) / (f64::from(self.maximum) - midpoint)
        };
        normalized.clamp(-1.0, 1.0) as f32
    }

    fn normalize_trigger(self, value: i32) -> f32 {
        if !self.supported {
            return 0.0;
        }
        // Treat the value observed when opening the device as its resting
        // trigger state. This handles centered or inverted ABS ranges without
        // mistaking their neutral point for a held trigger.
        let rest = f64::from(self.initial_value);
        let range = (rest - f64::from(self.minimum)).max(f64::from(self.maximum) - rest);
        if range <= 0.0 {
            return 0.0;
        }
        ((f64::from(value) - rest).abs() / range).clamp(0.0, 1.0) as f32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ControllerInputEvent {
    Key { code: u16, value: i32 },
    Absolute { code: u16, value: i32 },
    SynReport,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[allow(dead_code)] // The full standard physical vocabulary is for internal consumers.
pub(crate) enum ControllerButton {
    South,
    East,
    West,
    North,
    LeftShoulder,
    RightShoulder,
    LeftTriggerButton,
    RightTriggerButton,
    Select,
    Start,
    Guide,
    LeftStick,
    RightStick,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ControllerFrame {
    pub(crate) buttons: [u64; KEY_WORDS],
    pub(crate) changed_buttons: [u64; KEY_WORDS],
    pub(crate) dpad_x: i8,
    pub(crate) dpad_y: i8,
    pub(crate) left_stick: [f32; 2],
    pub(crate) right_stick: [f32; 2],
    pub(crate) triggers: [f32; 2],
    pub(crate) button_transition: bool,
    pub(crate) dpad_transition: bool,
    pub(crate) activity_transition: bool,
}

impl ControllerFrame {
    /// Returns the current physical state of a standard location-based button.
    /// D-pad buttons merge Linux BTN_DPAD codes with ABS_HAT0 state.
    pub(crate) fn standard_button_pressed(&self, button: ControllerButton) -> bool {
        let code = match button {
            ControllerButton::South => 0x130,
            ControllerButton::East => 0x131,
            ControllerButton::West => 0x134,
            ControllerButton::North => 0x133,
            ControllerButton::LeftShoulder => 0x136,
            ControllerButton::RightShoulder => 0x137,
            ControllerButton::LeftTriggerButton => 0x138,
            ControllerButton::RightTriggerButton => 0x139,
            ControllerButton::Select => 0x13a,
            ControllerButton::Start => 0x13b,
            ControllerButton::Guide => 0x13c,
            ControllerButton::LeftStick => 0x13d,
            ControllerButton::RightStick => 0x13e,
            ControllerButton::DpadUp => BTN_DPAD_UP,
            ControllerButton::DpadDown => 0x221,
            ControllerButton::DpadLeft => 0x222,
            ControllerButton::DpadRight => BTN_DPAD_RIGHT,
        };
        let index = usize::from(code);
        let key_pressed =
            self.buttons[index / u64::BITS as usize] & (1u64 << (index % u64::BITS as usize)) != 0;
        key_pressed
            || match button {
                ControllerButton::DpadUp => self.dpad_y < 0,
                ControllerButton::DpadDown => self.dpad_y > 0,
                ControllerButton::DpadLeft => self.dpad_x < 0,
                ControllerButton::DpadRight => self.dpad_x > 0,
                _ => false,
            }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ControllerFrameBuilder {
    ranges: [AxisRange; ABS_AXIS_COUNT],
    axes: [i32; ABS_AXIS_COUNT],
    buttons: [u64; KEY_WORDS],
    changed_buttons: [u64; KEY_WORDS],
    left_stick_active: bool,
    right_stick_active: bool,
    left_trigger_active: bool,
    right_trigger_active: bool,
    pending_button_transition: bool,
    pending_dpad_transition: bool,
    pending_changes: bool,
}

impl Default for ControllerFrameBuilder {
    fn default() -> Self {
        Self {
            ranges: [AxisRange::default(); ABS_AXIS_COUNT],
            axes: [0; ABS_AXIS_COUNT],
            buttons: [0; KEY_WORDS],
            changed_buttons: [0; KEY_WORDS],
            left_stick_active: false,
            right_stick_active: false,
            left_trigger_active: false,
            right_trigger_active: false,
            pending_button_transition: false,
            pending_dpad_transition: false,
            pending_changes: false,
        }
    }
}

impl ControllerFrameBuilder {
    pub(crate) fn new(ranges: [AxisRange; ABS_AXIS_COUNT]) -> Self {
        let mut builder = Self {
            ranges,
            ..Self::default()
        };
        for (axis, range) in ranges.into_iter().enumerate() {
            builder.axes[axis] = range.initial_value;
        }
        builder
    }

    pub(crate) fn seed_pressed_key(&mut self, code: u16) {
        if is_controller_button(code) {
            self.set_button(code, true);
            self.changed_buttons.fill(0);
        }
    }

    pub(crate) fn process(&mut self, event: ControllerInputEvent) -> Option<ControllerFrame> {
        match event {
            ControllerInputEvent::Key { code, value } => {
                self.process_key(code, value);
                None
            }
            ControllerInputEvent::Absolute { code, value } => {
                self.process_absolute(code, value);
                None
            }
            ControllerInputEvent::SynReport => self.finish_frame(),
            ControllerInputEvent::Other => None,
        }
    }

    pub(crate) fn clear_session_state(&mut self) {
        self.axes.fill(0);
        for (axis, range) in self.ranges.iter().enumerate() {
            self.axes[axis] = range.initial_value;
        }
        self.buttons.fill(0);
        self.changed_buttons.fill(0);
        self.left_stick_active = false;
        self.right_stick_active = false;
        self.left_trigger_active = false;
        self.right_trigger_active = false;
        self.pending_button_transition = false;
        self.pending_dpad_transition = false;
        self.pending_changes = false;
    }

    /// Captures current physical state without consuming pending transitions.
    pub(crate) fn state_snapshot(&self) -> ControllerFrame {
        ControllerFrame {
            buttons: self.buttons,
            changed_buttons: [0; KEY_WORDS],
            dpad_x: self.axes[usize::from(HAT0_X)].clamp(-1, 1) as i8,
            dpad_y: self.axes[usize::from(HAT0_Y)].clamp(-1, 1) as i8,
            left_stick: [self.signed_axis(ABS_X), self.signed_axis(ABS_Y)],
            right_stick: [self.signed_axis(ABS_RX), self.signed_axis(ABS_RY)],
            triggers: [self.trigger_axis(ABS_Z), self.trigger_axis(ABS_RZ)],
            button_transition: false,
            dpad_transition: false,
            activity_transition: false,
        }
    }

    fn process_key(&mut self, code: u16, value: i32) {
        if !is_controller_button(code) || value == 2 {
            return;
        }
        let pressed = value != 0;
        let index = usize::from(code);
        let word = index / u64::BITS as usize;
        let mask = 1u64 << (index % u64::BITS as usize);
        let was_pressed = self.buttons[word] & mask != 0;
        if was_pressed == pressed {
            return;
        }
        self.buttons[word] ^= mask;
        self.changed_buttons[word] |= mask;
        self.pending_changes = true;
        self.pending_button_transition = true;
        if is_dpad_button(code) {
            self.pending_dpad_transition = true;
        }
    }

    fn process_absolute(&mut self, code: u16, value: i32) {
        let index = usize::from(code);
        if index >= ABS_AXIS_COUNT || self.axes[index] == value {
            return;
        }
        self.axes[index] = value;
        self.pending_changes = true;
        if code == HAT0_X || code == HAT0_Y {
            self.pending_dpad_transition = true;
        }
    }

    fn finish_frame(&mut self) -> Option<ControllerFrame> {
        if !self.pending_changes {
            return None;
        }

        let left_stick = [self.signed_axis(ABS_X), self.signed_axis(ABS_Y)];
        let right_stick = [self.signed_axis(ABS_RX), self.signed_axis(ABS_RY)];
        let triggers = [self.trigger_axis(ABS_Z), self.trigger_axis(ABS_RZ)];
        let analog_activity = update_hysteresis(&mut self.left_stick_active, magnitude(left_stick))
            | update_hysteresis(&mut self.right_stick_active, magnitude(right_stick))
            | update_hysteresis(&mut self.left_trigger_active, triggers[0])
            | update_hysteresis(&mut self.right_trigger_active, triggers[1]);
        let dpad_x = self.axes[usize::from(HAT0_X)].clamp(-1, 1) as i8;
        let dpad_y = self.axes[usize::from(HAT0_Y)].clamp(-1, 1) as i8;
        let button_transition = self.pending_button_transition;
        let dpad_transition = self.pending_dpad_transition;
        let frame = ControllerFrame {
            buttons: self.buttons,
            changed_buttons: self.changed_buttons,
            dpad_x,
            dpad_y,
            left_stick,
            right_stick,
            triggers,
            button_transition,
            dpad_transition,
            activity_transition: button_transition || dpad_transition || analog_activity,
        };
        self.changed_buttons.fill(0);
        self.pending_button_transition = false;
        self.pending_dpad_transition = false;
        self.pending_changes = false;
        Some(frame)
    }

    fn signed_axis(&self, code: u16) -> f32 {
        let index = usize::from(code);
        self.ranges[index].normalize_signed(self.axes[index])
    }

    fn trigger_axis(&self, code: u16) -> f32 {
        let index = usize::from(code);
        self.ranges[index].normalize_trigger(self.axes[index])
    }

    fn set_button(&mut self, code: u16, pressed: bool) {
        let index = usize::from(code);
        let word = index / u64::BITS as usize;
        let mask = 1u64 << (index % u64::BITS as usize);
        if pressed {
            self.buttons[word] |= mask;
        } else {
            self.buttons[word] &= !mask;
        }
    }
}

pub(crate) const fn is_controller_button(code: u16) -> bool {
    (code >= 0x120 && code <= 0x13f)
        || (code >= BTN_DPAD_UP && code <= BTN_DPAD_RIGHT)
        || (code >= 0x2c0 && code <= 0x2ff)
}

const fn is_dpad_button(code: u16) -> bool {
    code >= BTN_DPAD_UP && code <= BTN_DPAD_RIGHT
}

fn magnitude(pair: [f32; 2]) -> f32 {
    (pair[0] * pair[0] + pair[1] * pair[1]).sqrt().min(1.0)
}

fn update_hysteresis(active: &mut bool, magnitude: f32) -> bool {
    if !*active && magnitude >= ACTIVITY_ENTER {
        *active = true;
        true
    } else {
        if *active && magnitude <= ACTIVITY_EXIT {
            *active = false;
        }
        false
    }
}
