#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum X11MotifDecorationHint {
    #[default]
    Unspecified,
    Decorated,
    Undecorated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct X11FrameExtents {
    pub left: u32,
    pub right: u32,
    pub top: u32,
    pub bottom: u32,
}

impl X11FrameExtents {
    pub const fn is_non_zero(self) -> bool {
        self.left != 0 || self.right != 0 || self.top != 0 || self.bottom != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct X11DecorationHints {
    pub motif: X11MotifDecorationHint,
    pub gtk_frame_extents: Option<X11FrameExtents>,
}
