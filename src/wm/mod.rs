pub mod layout;

mod special_workspace;
mod window;
mod workspace;

pub use crate::core::WindowId;
pub use special_workspace::{SPECIAL_WORKSPACE_COUNT, SpecialWorkspaceId, WorkspaceLocation};
pub use window::{
    LayoutMembership, WindowChromePolicy, WindowDecorationPolicy, WindowManagementState,
};
pub use workspace::{
    SpecialWorkspaceToggleOutcome, VisibleWorkspaces, WorkspaceId, WorkspaceManager,
    WorkspaceSwitchOutcome,
};
