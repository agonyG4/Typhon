mod window;
mod workspace;

pub use crate::core::WindowId;
pub use window::{LayoutMembership, WindowManagementState};
pub use workspace::{WorkspaceId, WorkspaceManager, WorkspaceSwitchOutcome};
