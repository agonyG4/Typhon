use super::{SpecialWorkspaceId, WorkspaceId, WorkspaceLocation};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMembership {
    Floating,
    Tiled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowChromePolicy {
    Full,
    Minimal,
}

impl WindowChromePolicy {
    pub const fn from_layout(layout: LayoutMembership) -> Self {
        match layout {
            LayoutMembership::Floating => Self::Full,
            LayoutMembership::Tiled => Self::Minimal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowManagementState {
    location: WorkspaceLocation,
    layout: LayoutMembership,
}

impl WindowManagementState {
    pub const fn new(location: WorkspaceLocation) -> Self {
        Self {
            location,
            layout: LayoutMembership::Floating,
        }
    }

    pub const fn location(self) -> WorkspaceLocation {
        self.location
    }

    pub const fn regular_workspace(self) -> Option<WorkspaceId> {
        match self.location {
            WorkspaceLocation::Regular(workspace) => Some(workspace),
            WorkspaceLocation::Special(_) => None,
        }
    }

    pub const fn special_workspace(self) -> Option<SpecialWorkspaceId> {
        match self.location {
            WorkspaceLocation::Regular(_) => None,
            WorkspaceLocation::Special(special) => Some(special),
        }
    }

    pub const fn with_location(self, location: WorkspaceLocation) -> Self {
        Self { location, ..self }
    }

    pub const fn layout(self) -> LayoutMembership {
        self.layout
    }

    pub const fn with_layout(self, layout: LayoutMembership) -> Self {
        Self { layout, ..self }
    }

    pub const fn chrome_policy(self) -> WindowChromePolicy {
        WindowChromePolicy::from_layout(self.layout)
    }
}
