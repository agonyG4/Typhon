#[derive(Debug, PartialEq)]
pub(super) enum ClientEvent {
    ReadyForPointer,
    MoveRequested,
    ResizeRequested,
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
    LockRequested,
    CursorState {
        pointer_motion_count: usize,
        relative_motion_count: usize,
        locked_count: usize,
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
        enters: Vec<Vec<u32>>,
        leaves: usize,
    },
}

#[derive(Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(super) enum ClientCommand {
    SetCursor,
    LockPointer,
    HideCursor,
    ReplaceCursor,
    CaptureCursorState,
    CaptureActive,
    BeginXdgMove,
    BeginXdgResize,
    CaptureButtons,
    CaptureKeyboard,
    CaptureKeyboardState,
    Finish,
}
