use super::*;

pub(crate) const WAYLAND_SCROLL_LINE_DISTANCE: f64 = 15.0;
pub(crate) const EV_KEY: u16 = 0x01;
pub(crate) const EV_REL: u16 = 0x02;
pub(crate) const KEY_1: u16 = 2;
pub(crate) const KEY_2: u16 = 3;
pub(crate) const KEY_3: u16 = 4;
pub(crate) const KEY_TAB: u16 = 15;
pub(crate) const KEY_Q: u16 = 16;
pub(crate) const KEY_P: u16 = 25;
pub(crate) const KEY_LEFTCTRL: u16 = 29;
pub(crate) const KEY_F: u16 = 33;
pub(crate) const KEY_LEFTSHIFT: u16 = 42;
#[cfg(test)]
pub(crate) const KEY_Z: u16 = 44;
pub(crate) const KEY_C: u16 = 46;
pub(crate) const KEY_RIGHTSHIFT: u16 = 54;
pub(crate) const KEY_LEFTALT: u16 = 56;
pub(crate) const KEY_SPACE: u16 = 57;
pub(crate) const KEY_F1: u16 = 59;
pub(crate) const KEY_F2: u16 = 60;
pub(crate) const KEY_F3: u16 = 61;
pub(crate) const KEY_F4: u16 = 62;
pub(crate) const KEY_F5: u16 = 63;
pub(crate) const KEY_F6: u16 = 64;
pub(crate) const KEY_F7: u16 = 65;
pub(crate) const KEY_F8: u16 = 66;
pub(crate) const KEY_F9: u16 = 67;
pub(crate) const KEY_F10: u16 = 68;
pub(crate) const KEY_F11: u16 = 87;
pub(crate) const KEY_F12: u16 = 88;
pub(crate) const KEY_RIGHTCTRL: u16 = 97;
pub(crate) const KEY_RIGHTALT: u16 = 100;
pub(crate) const KEY_LEFTMETA: u16 = 125;
pub(crate) const KEY_RIGHTMETA: u16 = 126;
pub(crate) const BTN_LEFT: u16 = 0x110;
pub(crate) const BTN_RIGHT: u16 = 0x111;
pub(crate) const BTN_MIDDLE: u16 = 0x112;
pub(crate) const REL_X: u16 = 0x00;
pub(crate) const REL_Y: u16 = 0x01;
pub(crate) const REL_HWHEEL: u16 = 0x06;
pub(crate) const REL_WHEEL: u16 = 0x08;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeKeyboardEvent {
    pub(crate) key: u32,
    pub(crate) pressed: bool,
}

impl NativeKeyboardEvent {
    pub(crate) const fn new(key: u16, pressed: bool) -> Self {
        Self {
            key: key as u32,
            pressed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct NativePointerButtonEvent {
    pub(crate) button: u32,
    pub(crate) pressed: bool,
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) output_width: u32,
    pub(crate) output_height: u32,
}

impl NativePointerButtonEvent {
    pub(crate) const fn new_at(
        button: u32,
        pressed: bool,
        x: f64,
        y: f64,
        output_width: u32,
        output_height: u32,
    ) -> Self {
        Self {
            button,
            pressed,
            x,
            y,
            output_width,
            output_height,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RelativeMotion {
    pub(crate) dx: f64,
    pub(crate) dy: f64,
    pub(crate) dx_unaccelerated: f64,
    pub(crate) dy_unaccelerated: f64,
}

impl RelativeMotion {
    pub(crate) const fn accelerated_only(dx: f64, dy: f64) -> Self {
        Self {
            dx,
            dy,
            dx_unaccelerated: dx,
            dy_unaccelerated: dy,
        }
    }

    pub(crate) const fn is_zero(self) -> bool {
        self.dx == 0.0
            && self.dy == 0.0
            && self.dx_unaccelerated == 0.0
            && self.dy_unaccelerated == 0.0
    }

    pub(crate) const fn add(self, other: Self) -> Self {
        Self {
            dx: self.dx + other.dx,
            dy: self.dy + other.dy,
            dx_unaccelerated: self.dx_unaccelerated + other.dx_unaccelerated,
            dy_unaccelerated: self.dy_unaccelerated + other.dy_unaccelerated,
        }
    }
}

impl From<RelativeMotion> for CompositorRelativePointerMotion {
    fn from(motion: RelativeMotion) -> Self {
        Self {
            dx: motion.dx,
            dy: motion.dy,
            dx_unaccelerated: motion.dx_unaccelerated,
            dy_unaccelerated: motion.dy_unaccelerated,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PointerMotionSample {
    pub(crate) timestamp_usec: u64,
    pub(crate) absolute: Option<(f64, f64)>,
    pub(crate) relative: Option<RelativeMotion>,
}

impl PointerMotionSample {
    pub(crate) const fn relative(timestamp_usec: u64, relative: RelativeMotion) -> Self {
        Self {
            timestamp_usec,
            absolute: None,
            relative: Some(relative),
        }
    }

    pub(crate) const fn absolute(timestamp_usec: u64, x: f64, y: f64) -> Self {
        Self {
            timestamp_usec,
            absolute: Some((x, y)),
            relative: None,
        }
    }

    pub(crate) fn coalesce(self, other: Self) -> Option<Self> {
        match (self.absolute, self.relative, other.absolute, other.relative) {
            (None, Some(left), None, Some(right)) => {
                Some(Self::relative(other.timestamp_usec, left.add(right)))
            }
            (Some(_), None, Some((x, y)), None) => Some(Self::absolute(other.timestamp_usec, x, y)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum NativeHardwareInputEvent {
    Key { code: u16, value: i32 },
    PointerButton { button: u32, pressed: bool },
    PointerMotion(PointerMotionSample),
    PointerAxis { horizontal: f64, vertical: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum NativeWindowAction {
    BeginMove {
        x: f64,
        y: f64,
        trigger_button: Option<u32>,
    },
    BeginResize {
        x: f64,
        y: f64,
        trigger_button: Option<u32>,
    },
    UpdateInteraction {
        x: f64,
        y: f64,
    },
    EndInteraction,
    CloseActiveWindow,
    ToggleFullscreen,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AstreaShortcutEvent {
    pub(crate) namespace: String,
    pub(crate) name: String,
    pub(crate) phase: AstreaShortcutPhase,
}

impl AstreaShortcutEvent {
    #[cfg(test)]
    pub(crate) fn pressed(namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
            phase: AstreaShortcutPhase::Pressed,
        }
    }
}

impl NativeHardwareInputEvent {
    pub(crate) const fn may_change_pointer_constraints(self) -> bool {
        matches!(self, Self::Key { .. } | Self::PointerButton { .. })
    }

    pub(crate) const fn timestamp_usec(self) -> Option<u64> {
        match self {
            Self::PointerMotion(sample) => Some(sample.timestamp_usec),
            Self::Key { .. } | Self::PointerButton { .. } | Self::PointerAxis { .. } => None,
        }
    }

    pub(crate) fn from_linux_event(event: LinuxInputEvent) -> Option<Self> {
        match event.type_ {
            EV_KEY if is_pointer_button(event.code) && event.value != 2 => {
                Some(Self::PointerButton {
                    button: u32::from(event.code),
                    pressed: event.value != 0,
                })
            }
            EV_KEY => Some(Self::Key {
                code: event.code,
                value: event.value,
            }),
            EV_REL => match event.code {
                REL_X => Some(Self::PointerMotion(PointerMotionSample::relative(
                    linux_input_event_time_usec(event),
                    RelativeMotion::accelerated_only(f64::from(event.value), 0.0),
                ))),
                REL_Y => Some(Self::PointerMotion(PointerMotionSample::relative(
                    linux_input_event_time_usec(event),
                    RelativeMotion::accelerated_only(0.0, f64::from(event.value)),
                ))),
                REL_WHEEL => Some(Self::PointerAxis {
                    horizontal: 0.0,
                    vertical: -f64::from(event.value) * WAYLAND_SCROLL_LINE_DISTANCE,
                }),
                REL_HWHEEL => Some(Self::PointerAxis {
                    horizontal: f64::from(event.value) * WAYLAND_SCROLL_LINE_DISTANCE,
                    vertical: 0.0,
                }),
                _ => None,
            },
            _ => None,
        }
    }
}

#[derive(Debug, Default, PartialEq)]
pub(crate) struct NativeInputEffect {
    pub(crate) redraw_requested: bool,
    pub(crate) visual_redraw_requested: bool,
    pub(crate) exit_requested: bool,
    pub(crate) cursor_moved: bool,
    pub(crate) cursor_position: Option<(i32, i32)>,
    pub(crate) keyboard_events: Vec<NativeKeyboardEvent>,
    pub(crate) pointer_motion: Option<(f64, f64)>,
    pub(crate) pointer_motion_usec: Option<u64>,
    pub(crate) relative_motion: Option<RelativeMotion>,
    pub(crate) pointer_buttons: Vec<NativePointerButtonEvent>,
    pub(crate) pointer_axis: Option<(f64, f64)>,
    pub(crate) window_actions: Vec<NativeWindowAction>,
    pub(crate) shortcut_events: Vec<AstreaShortcutEvent>,
    pub(crate) launch_command: Option<Vec<String>>,
    pub(crate) launch_source: Option<NativeLaunchSource>,
    pub(crate) vt_switch: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) enum NativePointerConstraintState {
    #[default]
    None,
    Locked {
        anchor: CompositorOutputPosition,
    },
    Confined {
        region: OutputRegion,
    },
}

impl NativePointerConstraintState {
    pub(crate) const fn locked(&self) -> bool {
        matches!(self, Self::Locked { .. })
    }

    pub(crate) fn constrain_position(
        &self,
        position: CompositorOutputPosition,
    ) -> CompositorOutputPosition {
        match self {
            Self::None | Self::Locked { .. } => position,
            Self::Confined { region } => region.closest_point(position),
        }
    }
}

impl NativeInputEffect {
    pub(crate) fn request_redraw(&mut self) {
        self.redraw_requested = true;
    }

    pub(crate) fn request_visual_redraw(&mut self) {
        self.redraw_requested = true;
        self.visual_redraw_requested = true;
    }

    pub(crate) fn mark_cursor_moved(&mut self, cursor_x: f64, cursor_y: f64) {
        self.cursor_moved = true;
        self.cursor_position = Some((cursor_x.round() as i32, cursor_y.round() as i32));
        self.request_redraw();
    }

    pub(crate) fn requires_frame_repaint(&self, cursor_mode: NativeCursorRenderMode) -> bool {
        if !self.redraw_requested {
            return false;
        }
        self.visual_redraw_requested
            || (cursor_mode == NativeCursorRenderMode::Software && self.cursor_moved)
    }
}
