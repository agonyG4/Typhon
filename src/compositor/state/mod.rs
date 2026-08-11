use super::*;

mod client_lifecycle;
mod cursor;
mod data_device;
mod desktop_windows;
mod dmabuf_feedback;
mod frame_callbacks;
mod frames;
mod fullscreen;
mod helpers;
mod hit_testing;
mod input_dispatch;
mod input_resources;
mod output_membership;
mod output_state;
mod override_redirect_stack;
mod pointer_constraints;
mod resize;
mod roles;
mod selection_runtime;
mod shortcuts;
mod shutdown;
mod subsurfaces;
mod support_types;
mod surface_commit_cursor;
mod surface_commit_placement;
mod surface_commits;
mod surface_focus;
mod surface_pacing;
mod surface_transactions;
mod surfaces;
mod window_actions;
mod window_interaction;
mod window_resize;
mod windows;
mod xdg_lifecycle;
mod xwayland_mode;
mod xwayland_scene;
mod xwayland_windows;

pub(crate) use override_redirect_stack::OverrideRedirectStackSnapshotResult;

#[allow(unused_imports)]
pub(in crate::compositor) use xwayland_scene::{
    XwaylandSceneBatchDirty, XwaylandSceneBatchMetrics, XwaylandSceneBatchState,
};
pub use xwayland_scene::{
    XwaylandSceneBatchError, XwaylandSceneBatchToken, XwaylandSceneMetricsSnapshot,
};

#[allow(unused_imports)]
pub(in crate::compositor) use client_lifecycle::*;
#[allow(unused_imports)]
pub(in crate::compositor) use cursor::*;
#[allow(unused_imports)]
pub(in crate::compositor) use data_device::*;
#[allow(unused_imports)]
pub(in crate::compositor) use desktop_windows::*;
#[allow(unused_imports)]
pub(in crate::compositor) use dmabuf_feedback::*;
#[allow(unused_imports)]
pub(crate) use frame_callbacks::*;
#[allow(unused_imports)]
pub(in crate::compositor) use frames::*;
#[allow(unused_imports)]
pub(in crate::compositor) use fullscreen::*;
#[allow(unused_imports)]
pub(in crate::compositor) use helpers::*;
#[allow(unused_imports)]
pub(in crate::compositor) use hit_testing::*;
#[allow(unused_imports)]
pub(in crate::compositor) use input_dispatch::*;
#[allow(unused_imports)]
pub(in crate::compositor) use input_resources::*;
#[allow(unused_imports)]
pub(in crate::compositor) use output_membership::*;
#[allow(unused_imports)]
pub(in crate::compositor) use output_state::*;
#[allow(unused_imports)]
pub(in crate::compositor) use pointer_constraints::*;
#[allow(unused_imports)]
pub(in crate::compositor) use resize::*;
pub(in crate::compositor) use roles::*;
#[allow(unused_imports)]
pub(in crate::compositor) use selection_runtime::*;
pub use shortcuts::AstreaShortcutPhase;
#[allow(unused_imports)]
pub(in crate::compositor) use shortcuts::*;
#[allow(unused_imports)]
pub(in crate::compositor) use shutdown::*;
#[allow(unused_imports)]
pub(in crate::compositor) use subsurfaces::*;
#[allow(unused_imports)]
pub(in crate::compositor) use support_types::*;
#[allow(unused_imports)]
pub(in crate::compositor) use surface_commit_cursor::*;
#[allow(unused_imports)]
pub(in crate::compositor) use surface_commit_placement::*;
#[allow(unused_imports)]
pub(in crate::compositor) use surface_commits::*;
#[allow(unused_imports)]
pub(in crate::compositor) use surface_focus::*;
pub use surface_pacing::SurfacePacingMetrics;
#[allow(unused_imports)]
pub(in crate::compositor) use surface_pacing::{
    ActiveFifoBarrier, CapturedSurfacePacing, CommitTimingConstraint, FifoBarrierClaim,
    FifoBarrierClearReason, FifoBarrierGeneration, PendingSurfacePacingState,
};
#[allow(unused_imports)]
pub(in crate::compositor) use surface_transactions::*;
#[allow(unused_imports)]
pub(in crate::compositor) use surfaces::*;
#[allow(unused_imports)]
pub(in crate::compositor) use window_actions::*;
#[allow(unused_imports)]
pub(in crate::compositor) use window_interaction::*;
#[allow(unused_imports)]
pub(in crate::compositor) use window_resize::*;
#[allow(unused_imports)]
pub(in crate::compositor) use windows::*;
#[allow(unused_imports)]
pub(in crate::compositor) use xdg_lifecycle::*;
#[allow(unused_imports)]
pub(in crate::compositor) use xwayland_mode::*;
#[allow(unused_imports)]
pub(in crate::compositor) use xwayland_scene::*;
#[allow(unused_imports)]
pub(in crate::compositor) use xwayland_windows::*;

#[cfg(test)]
mod desktop_window_tests;
#[cfg(test)]
mod frame_tests;
#[cfg(test)]
mod task_05_8_tests;
#[cfg(test)]
mod window_interaction_tests;
