#[derive(Debug, PartialEq)]
pub(super) enum ClientEvent {
    ReadyForPointer,
    MoveRequested,
    Buttons {
        pressed_count: usize,
        released_count: usize,
    },
    Active {
        pointer_motion_count: usize,
        pointer_surface_x: Option<f64>,
        pointer_surface_y: Option<f64>,
        pointer_enter_count: usize,
        pointer_leave_count: usize,
    },
    CursorReady {
        pointer_motion_count: usize,
        pointer_enter_count: usize,
        pointer_leave_count: usize,
    },
    Finished {
        pointer_motion_count: usize,
        pointer_surface_x: Option<f64>,
        pointer_surface_y: Option<f64>,
        pointer_enter_count: usize,
        pointer_leave_count: usize,
    },
    KeyboardGroups {
        groups: Vec<u32>,
    },
    KeyboardState {
        keymap: Vec<u8>,
        keys: Vec<(u32, bool)>,
        modifiers: Vec<(u32, u32, u32, u32)>,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ClientCommand {
    SetCursor,
    CaptureActive,
    BeginXdgMove,
    CaptureButtons,
    CaptureKeyboard,
    CaptureKeyboardState,
    Finish,
}
