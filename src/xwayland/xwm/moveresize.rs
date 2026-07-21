#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X11MoveResizeDirection {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
    Move,
    KeyboardSize,
    KeyboardMove,
    Cancel,
}

impl X11MoveResizeDirection {
    pub(super) const fn from_ewmh(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::TopLeft),
            1 => Some(Self::Top),
            2 => Some(Self::TopRight),
            3 => Some(Self::Right),
            4 => Some(Self::BottomRight),
            5 => Some(Self::Bottom),
            6 => Some(Self::BottomLeft),
            7 => Some(Self::Left),
            8 => Some(Self::Move),
            9 => Some(Self::KeyboardSize),
            10 => Some(Self::KeyboardMove),
            11 => Some(Self::Cancel),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct X11MoveResizeRequest {
    pub root_x: i32,
    pub root_y: i32,
    pub direction: X11MoveResizeDirection,
    pub button: u32,
    pub source: u32,
}
