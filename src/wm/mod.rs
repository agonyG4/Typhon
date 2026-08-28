pub mod layout;

mod special_workspace;
mod window;
mod workspace;

pub use crate::core::WindowId;
pub use special_workspace::{SpecialWorkspaceId, WorkspaceLocation};
pub use window::{LayoutMembership, WindowChromePolicy, WindowManagementState};
pub use workspace::{
    SpecialWorkspaceToggleOutcome, WorkspaceId, WorkspaceManager, WorkspaceSwitchOutcome,
};
