use super::WorkspaceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMembership {
    Floating,
    Tiled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowManagementState {
    workspace: WorkspaceId,
    layout: LayoutMembership,
}

impl WindowManagementState {
    pub const fn new(workspace: WorkspaceId) -> Self {
        Self {
            workspace,
            layout: LayoutMembership::Floating,
        }
    }

    pub const fn workspace(self) -> WorkspaceId {
        self.workspace
    }

    pub const fn with_workspace(self, workspace: WorkspaceId) -> Self {
        Self { workspace, ..self }
    }

    pub const fn layout(self) -> LayoutMembership {
        self.layout
    }

    pub const fn with_layout(self, layout: LayoutMembership) -> Self {
        Self { layout, ..self }
    }
}
