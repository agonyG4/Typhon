use super::*;

mod active_scene;
mod client_lifecycle;
mod commit_timing_runtime;
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
mod scene_order;
mod scene_work;
mod selection_runtime;
mod shortcut_inhibition;
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
mod surface_tree_readiness;
mod surfaces;
mod tiled_layout;
mod tiled_resize;
mod window_actions;
mod window_decoration;
mod window_interaction;
mod window_resize;
mod windows;
mod workspaces;
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
pub(in crate::compositor) use active_scene::*;
#[allow(unused_imports)]
pub(in crate::compositor) use client_lifecycle::*;
pub(in crate::compositor) use commit_timing_runtime::CommitTimingTargetClaim;
pub use commit_timing_runtime::{
    CommitTimingClockMappingMetadata, CommitTimingClockSample, CommitTimingPlanningCandidate,
    CommitTimingReadiness, CommitTimingSchedulerDeadline,
};
#[allow(unused_imports)]
pub(super) use commit_timing_runtime::{
    CommitTimingRevalidation, revalidate_commit_timing_readiness, timestamp_as_nanos_u128,
};
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
pub(in crate::compositor) use scene_order::*;
#[allow(unused_imports)]
pub(in crate::compositor) use scene_work::*;
#[allow(unused_imports)]
pub(in crate::compositor) use selection_runtime::*;
pub(in crate::compositor) use shortcut_inhibition::*;
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
#[allow(unused_imports)]
pub(in crate::compositor) use surface_pacing::{
    ActiveFifoBarrier, CapturedSurfacePacing, FifoBarrierClaim, FifoBarrierClearReason,
    FifoBarrierGeneration, PendingSurfacePacingState,
};
pub use surface_pacing::{CommitTimingConstraint, SurfacePacingMetrics};
pub use surface_transactions::SurfaceTreeTransactionId;
#[allow(unused_imports)]
pub(in crate::compositor) use surface_transactions::{
    BufferlessSurfaceCommitState, PendingSurfaceTreeTransaction, ReleasedSurfaceTreeState,
    SurfacePublicationContext, SurfacePublicationDecision, SurfacePublicationSource,
    SurfacePublicationState, SurfaceTreeAcquireDependency, SurfaceTreeMergeStats,
    TransactionOrdering,
};
#[allow(unused_imports)]
pub(in crate::compositor) use surfaces::*;
#[allow(unused_imports)]
pub(in crate::compositor) use tiled_resize::*;
#[allow(unused_imports)]
pub(in crate::compositor) use window_actions::*;
#[allow(unused_imports)]
pub(in crate::compositor) use window_decoration::*;
#[allow(unused_imports)]
pub(in crate::compositor) use window_interaction::*;
#[allow(unused_imports)]
pub(in crate::compositor) use window_resize::*;
#[allow(unused_imports)]
pub(in crate::compositor) use windows::*;
#[allow(unused_imports)]
pub(in crate::compositor) use workspaces::*;
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
mod tiled_layout_tests;
#[cfg(test)]
mod window_decoration_tests;
#[cfg(test)]
mod window_interaction_tests;
