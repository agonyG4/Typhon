#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X11WindowType {
    Normal,
    Dialog,
    Utility,
    Menu,
    PopupMenu,
    DropdownMenu,
    Tooltip,
    Notification,
    Combo,
    Splash,
    Toolbar,
    Dock,
    Desktop,
    Dnd,
    Other(u32),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct X11WindowTypes {
    pub atoms: Vec<X11WindowType>,
}

impl X11WindowTypes {
    pub fn new(atoms: Vec<X11WindowType>) -> Self {
        Self { atoms }
    }

    pub fn preferred_supported_type(&self) -> Option<X11WindowType> {
        self.atoms
            .iter()
            .copied()
            .find(|window_type| !matches!(window_type, X11WindowType::Other(_)))
    }

    pub fn contains(&self, window_type: X11WindowType) -> bool {
        self.atoms.contains(&window_type)
    }
}
