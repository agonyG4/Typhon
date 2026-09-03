use crate::astrea_shell_control::server::astrea_launch_request_v1;
use crate::astrea_shortcuts::server::astrea_shortcut_v1;
use crate::render_backend::buffer::{
    BufferId, BufferIdAllocator, BufferIdentity, BufferSize, DmabufBufferHandle,
    DmabufPlane as RenderDmabufPlane, DmabufPlaneDescriptor, DrmFormat, DrmModifier,
};
use crate::render_backend::egl_gles::EglGlesDmabufFeedback;
use crate::syncobj::DrmSyncobjDevice;
use crate::wayland_drm::server::wl_drm;
use crate::wm::layout::{LayoutGeneration, TiledLayoutManager};
use crate::wm::{WorkspaceLocation, WorkspaceManager};
use crate::xwayland::{X11WindowHandle, XwaylandGeneration};
pub use clipboard_bridge::{
    ClipboardBridge, ClipboardBridgeError, ClipboardBridgeEvent, HostClipboardOfferId,
    NoopClipboardBridge,
};
use gpu_protocol_capabilities::GpuProtocolCapabilities;
use std::{
    cell::Cell,
    collections::{HashMap, HashSet, VecDeque},
    fs::File,
    io,
    os::fd::{AsFd, OwnedFd},
    sync::{Arc, Mutex},
    time::Instant,
};
use wayland_protocols::ext::data_control::v1::server::{
    ext_data_control_device_v1, ext_data_control_manager_v1, ext_data_control_offer_v1,
    ext_data_control_source_v1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::server::{
    zwp_linux_buffer_params_v1, zwp_linux_dmabuf_feedback_v1, zwp_linux_dmabuf_v1,
};
use wayland_protocols::wp::linux_drm_syncobj::v1::server::{
    wp_linux_drm_syncobj_manager_v1, wp_linux_drm_syncobj_surface_v1,
    wp_linux_drm_syncobj_timeline_v1,
};
use wayland_protocols::wp::{
    commit_timing::v1::server::{wp_commit_timer_v1, wp_commit_timing_manager_v1},
    content_type::v1::server::{wp_content_type_manager_v1, wp_content_type_v1},
    fifo::v1::server::{wp_fifo_manager_v1, wp_fifo_v1},
    fractional_scale::v1::server::{wp_fractional_scale_manager_v1, wp_fractional_scale_v1},
    idle_inhibit::zv1::server::{zwp_idle_inhibit_manager_v1, zwp_idle_inhibitor_v1},
    pointer_constraints::zv1::server::{
        zwp_confined_pointer_v1, zwp_locked_pointer_v1, zwp_pointer_constraints_v1,
    },
    pointer_warp::v1::server::wp_pointer_warp_v1,
    presentation_time::server::{wp_presentation, wp_presentation_feedback},
    primary_selection::zv1::server::{
        zwp_primary_selection_device_manager_v1, zwp_primary_selection_device_v1,
        zwp_primary_selection_offer_v1, zwp_primary_selection_source_v1,
    },
    relative_pointer::zv1::server::{zwp_relative_pointer_manager_v1, zwp_relative_pointer_v1},
    tearing_control::v1::server::{wp_tearing_control_manager_v1, wp_tearing_control_v1},
    viewporter::server::{wp_viewport, wp_viewporter},
};
use wayland_protocols::xdg::{
    activation::v1::server::{xdg_activation_token_v1, xdg_activation_v1},
    decoration::zv1::server::{zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1},
    shell::server::{xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base},
};
use wayland_protocols_wlr::layer_shell::v1::server::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};
use wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
    backend::{ClientId, ObjectId},
    protocol::{
        wl_buffer, wl_callback, wl_compositor, wl_data_device, wl_data_device_manager,
        wl_data_offer, wl_data_source, wl_keyboard, wl_output, wl_pointer, wl_region, wl_seat,
        wl_shm, wl_shm_pool, wl_subcompositor, wl_subsurface, wl_surface,
    },
};
mod astrea_shell_capability;
mod clipboard_bridge;
mod color;
mod commit_debug;
mod decoration;
mod desktop_window;
mod dmabuf;
mod explicit_sync;
mod frame_batch;
mod fullscreen;
pub mod gpu_protocol_capabilities;
mod idle;
mod input;
mod interaction;
mod layer_shell;
mod output;
mod pacing;
mod plan;
mod popup;
mod presentation;
mod presentation_modes;
mod protocols;
pub(crate) use protocols::cursor_shape::ProtocolCursorShape;
mod render;
mod runtime_files;
mod selection;
mod server;
mod server_backend;
mod server_error;
mod server_frames;
mod server_interaction;
mod server_toplevel;
mod server_xwayland_events;
mod shm;
mod state_data;
mod state_data_presentation;
mod subsurface;
mod surface;
mod surface_pacing_data;
mod toplevel_actions;
mod toplevel_collection;
mod toplevel_publication;
mod toplevel_publication_state;
mod window_backend;
mod window_state;
mod workspace_protocol;
pub use crate::core::WindowId;
use commit_debug::*;
pub use desktop_window::{
    DesktopStackLayer, DesktopWindowKind, WindowConstraints, WindowMetadata, X11DesktopRole,
    X11PlacementPolicy,
};
#[allow(unused_imports)]
pub(crate) use desktop_window::{
    DesktopWindow, DesktopWindowError, WindowBackend, WindowRelationships, XdgWindowHandle,
    classify_x11_role, x11_placement_policy,
};
pub use dmabuf::{DirectScanoutFeedbackCapabilities, DirectScanoutFormatCapability};
use dmabuf::{
    DmabufBufferData, DmabufFeedbackData, DmabufParamsData, PendingDmabufPlane,
    send_dmabuf_feedback, send_dmabuf_format_modifiers, send_wl_drm_capabilities,
};
#[doc(hidden)]
pub use explicit_sync::{
    AcquireCommitId, AcquireWatchCancelReason, AcquireWatchChange, AcquireWatchRequest,
    ExplicitSyncPoint,
};
use explicit_sync::{
    AcquireCommitIdAllocator, CapturedExplicitSyncState, PendingAcquireState,
    PendingExplicitSyncCommit, PendingPresentationFeedback, SYNCOBJ_MANAGER_ERROR_INVALID_TIMELINE,
    SYNCOBJ_MANAGER_ERROR_SURFACE_EXISTS, SYNCOBJ_SURFACE_ERROR_CONFLICTING_POINTS,
    SYNCOBJ_SURFACE_ERROR_NO_ACQUIRE_POINT, SYNCOBJ_SURFACE_ERROR_NO_BUFFER,
    SYNCOBJ_SURFACE_ERROR_NO_RELEASE_POINT, SYNCOBJ_SURFACE_ERROR_NO_SURFACE,
    SYNCOBJ_SURFACE_ERROR_UNSUPPORTED_BUFFER, SyncobjSurfaceState, SyncobjTimelineData,
};
pub(crate) use frame_batch::FrameCallbackSettlement;
pub use frame_batch::{
    BufferReleaseMetrics, CompositorFrameBatchId, DmabufGpuReleaseLeaseId, FrameCallbackAdmission,
    FrameCallbackMetrics, FrameCallbackTimingEvidence,
};
pub(crate) use frame_batch::{CompositorFrameBatch, DmabufGpuReleaseLease};
pub use state_data::ShmBufferLifetimeMetrics;
pub(in crate::compositor) use state_data::{CurrentSurfaceBuffer, DmabufReleaseObligation};
#[allow(unused_imports)]
pub(in crate::compositor) use toplevel_publication::*;
use workspace_protocol::WorkspaceProtocolState;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalCallbackDisposition {
    Presented,
    NoVisualChange,
    Retryable,
    Superseded,
    Cancelled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalCallbackOwnership {
    None,
    Resolved {
        completed: usize,
    },
    Transferred {
        owner: CompositorFrameBatchId,
        callbacks: usize,
    },
    Cancelled {
        callbacks: usize,
    },
    Leaked {
        owner: CompositorFrameBatchId,
        unresolved: usize,
        reason: TerminalCallbackLeakReason,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalCallbackLeakReason {
    MissingBatch,
    CountMismatch,
    UnresolvedAtTerminal,
    MissingTransferTarget,
}
pub use decoration::raster::DecorationRasterAsset;
pub use decoration::render_plan::DecorationRenderPrimitive;
use decoration::theme::DecorationThemeSnapshot;
use decoration::types::DecorationButtonKind;
pub use decoration::types::DecorationRect;
pub use fullscreen::DirectScanoutSceneBlockers;
pub(crate) use fullscreen::direct_scanout_scene_rejection_for_flags;
pub use fullscreen::{
    DirectScanoutSceneCandidate, DirectScanoutSceneRejection, FullscreenPresentationEligibility,
    FullscreenPresentationRejection, FullscreenPresentationState, FullscreenRenderPlanMetrics,
};
pub use idle::{IdleManager, IdleState};
use input::{
    InputSerial, InputSerialKind, KeyboardModifierState, PointerConstraintLifetime,
    send_keyboard_initial_state, send_pointer_frame_if_supported, wayland_event_time,
    wayland_event_time_from_usec,
};
pub use input::{
    OutputPosition, OutputRect, OutputRegion, PointerAxisComponent, PointerAxisFrame,
    PointerAxisSource, PointerConstraintBackendId, PointerConstraintBackendRequest,
    PointerConstraintMode, PointerConstraintState, PointerMotionSample, RelativePointerMotion,
};
pub use interaction::X11MoveResizeBeginResult;
use interaction::{
    InteractionCursorOverride, InteractionCursorShape, PendingFloatingResize,
    PendingResizeConfigure, PointerPress, PointerTarget, ResizeAckDecision, ResizeCommitSnapshot,
    ResizeConfigureFlow, ResizeEdges, RootSurfaceHit, WindowFrameHit, WindowInteraction,
    WindowInteractionEndReason, WindowInteractionSource, interactive_resize_geometry,
    resize_drag_threshold_reached, resize_edges_for_window_point, resize_edges_from_xdg,
    window_frame_action_for_local_point,
};
pub use interaction::{
    InteractionUpdateOutcome, ResizeInteractionId, TriggerReleaseDelivery,
    WindowInteractionButtonRelease, WindowInteractionDebugSnapshot, WindowInteractionId,
    WindowInteractionKind, WindowInteractionReleaseContext, WindowInteractionReleaseDebugRecord,
    WindowInteractionReleaseMetrics,
};
use layer_shell::{Layer, LayerSurfaceRole};
use output::{
    OutputRefreshRate, OutputScale, OutputSize, send_output_description,
    send_output_done_if_supported, send_output_mode, send_output_scale,
};
use pacing::*;
pub use plan::*;
use popup::{
    PopupAnchorRect, PopupConstraintAdjustment, PopupEdges, PopupRect, XdgPositionerState,
    XdgWindowGeometry,
};
pub use presentation::{
    FramePresentation, PresentationClock, PresentationKind, PresentationTimestamp,
};
pub use presentation_modes::*;
pub use render::{
    BufferAge, DecorationRenderInstance, DecorationSceneSnapshot, DesktopComposeRequest,
    DesktopFrameCopyKind, DesktopSceneRebuildKind, DesktopSceneRenderer, DesktopVisualState,
    OUTPUT_BACKGROUND, RenderSceneElement, RenderSceneElementId, RenderSceneElementKind,
    ServerFrameColor, SurfaceRenderPlan, SurfaceRenderSpaceAssignment, SurfaceTargetRect,
    SurfaceVisualAperture, VisualStackGroup, WindowVisualGroup, compose_output, cursor_damage_rect,
    output_scale_key, render_scene_elements_for_surfaces, scale_desktop_visual_state,
    scale_logical_coordinate, scale_logical_extent, server_frame_rects_by_surface,
    server_frame_rects_for_surface, surface_origin, surface_origins, surface_render_plan,
    surface_render_plan_with_clip, surface_render_plans_with_aperture,
    surface_render_space_assignments, visual_stack_groups, window_visual_stack_order,
    xwayland_visual_backing_target,
};
use runtime_files::{compositor_debug_surface_logging_enabled, unique_runtime_file_path};
pub use runtime_files::{resize_debug_log, resize_debug_logging_enabled};
pub use selection::*;
pub use server::{OwnCompositorServer, XwaylandClientIdentity};
pub use server_error::CompositorError;
use shm::{
    ShmBufferData, ShmPoolData, WL_SHM_FORMAT_ABGR8888, WL_SHM_FORMAT_ABGR2101010,
    WL_SHM_FORMAT_ARGB2101010, WL_SHM_FORMAT_XBGR8888, WL_SHM_FORMAT_XBGR2101010,
    WL_SHM_FORMAT_XRGB2101010, shm_format_descriptor,
};
pub(crate) use state::OverrideRedirectStackSnapshotResult;
pub use state::{
    AstreaShortcutPhase, CommitTimingClockMappingMetadata, CommitTimingClockSample,
    CommitTimingConstraint, CommitTimingPlanningCandidate, CommitTimingReadiness,
    CommitTimingSchedulerDeadline, SurfacePacingMetrics, SurfaceTreeTransactionId,
    XwaylandSceneBatchError, XwaylandSceneBatchToken, XwaylandSceneMetricsSnapshot,
};
use state_data::*;
use subsurface::{
    CachedSubsurfaceCommit, CapturedPointerConstraintSurfaceState, PointerConstraintHintCommit,
    PointerConstraintLifecycleCommit, PointerConstraintRegionCommit, SubsurfaceSyncMode,
    SubsurfaceTransactionState,
};
pub use surface::{
    DamageSince, RenderableSurface, RenderableSurfaceDamage, RootPlacementMode,
    SurfaceCommitCounter, SurfaceCommitSequence, SurfaceDamageJournal, SurfaceDamageRect,
    SurfaceOpaqueRect, SurfaceOpaqueRegion, SurfacePlacement, SurfaceRenderBackend,
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameCallbackTime(u32);
impl FrameCallbackTime {
    pub const fn new(milliseconds: u32) -> Self {
        Self(milliseconds)
    }
    pub const fn milliseconds(self) -> u32 {
        self.0
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolOnlyCompletion {
    Completed { callback_count: usize },
    NoCallbacks,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XwaylandSurfaceCommitObserved {
    pub generation: XwaylandGeneration,
    pub surface_id: u32,
    pub association_serial: std::num::NonZeroU64,
    pub commit_sequence: SurfaceCommitSequence,
    pub buffer_id: Option<BufferId>,
    pub buffer_size: Option<BufferSize>,
}
use window_state::{ToplevelMode, WindowGeometry, WindowState, xdg_toplevel_state_bytes};
const MIN_WINDOW_WIDTH: u32 = 160;
const MIN_WINDOW_HEIGHT: u32 = 120;
const WL_SEAT_NAME_SINCE: u32 = 2;
#[cfg(test)]
const DRM_FORMAT_ARGB8888: u32 = DrmFormat::ARGB8888_FOURCC;
#[cfg(test)]
const DRM_FORMAT_MOD_LINEAR: u64 = DrmModifier::LINEAR.0;
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ResizeFlowMetrics {
    pub configures_requested: u64,
    pub configures_sent: u64,
    pub geometries_coalesced: u64,
    pub acks_matched: u64,
    pub acks_stale: u64,
    pub acks_unknown: u64,
    pub resize_acks_replaced_uncaptured: u64,
    pub resize_acks_preserved_while_captures_pending: u64,
    pub commits_captured: u64,
    pub resize_captures_pending: usize,
    pub resize_captures_pending_peak: usize,
    pub resize_captures_completed: u64,
    pub resize_captures_released: u64,
    pub resize_configure_capacity_blocked: u64,
    pub resize_xdg_geometry_rejected_as_content_size: u64,
    pub commits_delayed_by_explicit_sync: u64,
    pub preview_activations: u64,
    pub preview_completions: u64,
    pub resize_interactions_started: u64,
    pub rapid_reresize_interactions: u64,
    pub obsolete_finals_discarded: u64,
    pub obsolete_queued_targets_discarded: u64,
    pub obsolete_in_flight_configures_discarded: u64,
    pub stale_interaction_commits_applied: u64,
    pub stale_commits_preserved_preview: u64,
    pub preview_ownership_transfers: u64,
    pub final_configures_sent: u64,
    pub resize_interactions_completed: u64,
    pub resize_interactions_canceled: u64,
    pub visual_geometry_resize_starts: u64,
    pub raw_pointer_move_updates: u64,
    pub pending_move_updates_replaced: u64,
    pub interactive_visual_work_queued_edges: u64,
    pub interactive_input_cycles_while_pending: u64,
    pub interactive_scheduler_decisions_while_pending: u64,
    pub interactive_render_admissions: u64,
    pub interactive_render_ahead_admissions: u64,
    pub interactive_non_render_admission_attempts: u64,
    pub move_updates_applied: u64,
    pub move_updates_skipped_unchanged: u64,
    pub raw_pointer_resize_updates: u64,
    pub pending_resize_updates_replaced: u64,
    pub resize_updates_applied: u64,
    pub resize_updates_skipped_unchanged: u64,
    pub floating_geometry_frame_flushes: u64,
    pub floating_geometry_terminal_flushes: u64,
    pub floating_geometry_stale_drops: u64,
    pub duplicate_configure_sizes_skipped: u64,
    pub maximum_retained_configures: usize,
    pub max_preview_age_ms: u64,
    pub max_in_flight_configures: usize,
    pub max_pending_explicit_sync_commits: usize,
    pub surface_content_publishes: u64,
    pub surface_content_stale_rejections: u64,
    pub surface_pending_attachments_superseded: u64,
    pub surface_cross_queue_supersessions: u64,
    pub surface_publication_sequence_regressions: u64,
    pub surface_sampling_exact: u64,
    pub surface_sampling_scaled: u64,
    pub x11_moveresize_began: u64,
    pub x11_moveresize_no_pressed_button: u64,
    pub x11_moveresize_button_mismatch: u64,
    pub x11_moveresize_stale_request: u64,
    pub tiled_resize_interactions_started: u64,
    pub tiled_resize_raw_updates: u64,
    pub tiled_resize_pending_replaced: u64,
    pub tiled_resize_frame_flushes: u64,
    pub tiled_resize_ratio_clamps: u64,
    pub tiled_resize_unchanged_flushes: u64,
    pub tiled_resize_frame_snapshot_windows: u64,
    pub tiled_resize_frame_node_visits: u64,
    pub tiled_constraint_reflows: u64,
    pub tiled_constraint_auto_floats: u64,
    pub tiled_migration_fallbacks: u64,
    pub tiled_work_area_reflows: u64,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SubsurfaceTransactionMetrics {
    pub synchronized_child_commits_cached: u64,
    pub cached_commits_merged: u64,
    pub tree_transactions_prepared: u64,
    pub tree_transactions_published: u64,
    pub tree_transactions_waiting_on_acquire: u64,
    pub tree_transactions_superseded: u64,
    pub explicit_sync_queue_overflow: u64,
    pub all_ready_queue_pressure: u64,
    pub bufferless_tree_commits_merged: u64,
    pub metadata_only_nodes_merged: u64,
    pub attachments_replaced: u64,
    pub explicit_detaches: u64,
    pub acquire_dependencies_preserved: u64,
    pub acquire_dependencies_replaced: u64,
    pub ready_transactions_preserved_from_newer_unready: u64,
    pub ready_transactions_preserved_from_newer_ready: u64,
    pub callbacks_merged: u64,
    pub feedbacks_merged: u64,
    pub resize_snapshots_preserved: u64,
    pub resize_snapshots_replaced: u64,
    pub root_wide_supersessions: u64,
    pub waiting_transactions_published: u64,
    pub maximum_ready_slots_per_root: usize,
    pub maximum_waiting_slots_per_root: usize,
    pub maximum_explicit_sync_queue_depth: usize,
    pub maximum_cached_nodes: usize,
    pub maximum_tree_depth: usize,
    pub maximum_transaction_wait_ms: u64,
    pub synchronized_child_immediate_publish_attempts: u64,
    pub surface_tree_publications: u64,
    pub surface_tree_stale_rejections: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ToplevelVisualGeometry {
    placement: SurfacePlacement,
    width: u32,
    height: u32,
    active_resize: Option<ResizeInteractionId>,
    mode_transition: bool,
}
impl ToplevelVisualGeometry {
    const fn window_geometry(self) -> WindowGeometry {
        WindowGeometry::new(self.placement, self.width, self.height)
    }
}
#[derive(Debug, Clone, Copy)]
struct ActiveToplevelResize {
    interaction_id: ResizeInteractionId,
    flow_sequence: u64,
    edges: ResizeEdges,
    activated_at: Instant,
}
#[derive(Debug, Default, Clone, Copy)]
struct XdgConfigureSerialState {
    latest_sent: u32,
    latest_acked: u32,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum RenderGenerationCause {
    #[default]
    Initial,
    SurfaceCommit,
    SurfaceDamage,
    SurfaceUnmap,
    SurfacePlacement,
    WindowMove,
    WindowResize,
    WindowMode,
    LayoutReflow,
    WindowMinimize,
    WindowRestore,
    WindowStack,
    WorkspaceSwitch,
    WorkspaceMove,
    OutputChange,
    CursorCommit,
    CursorMotion,
    CursorState,
    WindowDecoration,
}
impl RenderGenerationCause {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::SurfaceCommit => "surface_commit",
            Self::SurfaceDamage => "surface_damage",
            Self::SurfaceUnmap => "surface_unmap",
            Self::SurfacePlacement => "surface_placement",
            Self::WindowMove => "window_move",
            Self::WindowResize => "window_resize",
            Self::WindowMode => "window_mode",
            Self::LayoutReflow => "layout_reflow",
            Self::WindowMinimize => "window_minimize",
            Self::WindowRestore => "window_restore",
            Self::WindowStack => "window_stack",
            Self::WorkspaceSwitch => "workspace_switch",
            Self::WorkspaceMove => "workspace_move",
            Self::OutputChange => "output_change",
            Self::CursorCommit => "cursor_commit",
            Self::CursorMotion => "cursor_motion",
            Self::CursorState => "cursor_state",
            Self::WindowDecoration => "window_decoration",
        }
    }
    pub const fn uses_surface_damage(self) -> bool {
        matches!(self, Self::SurfaceCommit | Self::SurfaceDamage)
    }
}
#[derive(Debug, Clone, Copy)]
pub struct ClientCursorRenderState<'a> {
    pub surface: &'a RenderableSurface,
    pub logical_x: i32,
    pub logical_y: i32,
    pub hotspot_x: i32,
    pub hotspot_y: i32,
}

#[doc(hidden)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceLocalityMetrics {
    pub global_renderable_index_rebuilds: u64,
    pub global_indexed_lookups: u64,
    pub content_indexed_lookups: u64,
    pub presentation_sampled_entries: u64,
    pub presentation_journal_lookups: u64,
    pub presentation_settlement_entries: u64,
    pub presentation_settlement_journal_lookups: u64,
    pub presentation_global_scans: u64,
    pub damage_authoritative_empty: u64,
    pub damage_history_lost_repairs: u64,
    pub surface_damage_settlement_presented: u64,
    pub surface_damage_settlement_no_visual_change: u64,
    pub xwayland_content_replacements: u64,
    pub xwayland_topology_reorders: u64,
    pub xwayland_full_visual_reassignments: u64,
    pub cursor_surface_samples_software: u64,
    pub cursor_surface_samples_hardware: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(in crate::compositor) enum SurfaceTeardownReason {
    ExplicitDestroy,
    ClientDisconnected,
    ProtocolError,
    RoleDestroyed,
    CompositorShutdown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct SurfaceTeardownResult {
    pub removed_resource: bool,
    pub removed_renderables: usize,
}
#[derive(Debug, Default)]
pub struct CompositorState {
    pub accepted_clients: usize,
    pub xdg_toplevels: usize,
    pub xdg_popups: usize,
    pub last_app_id: Option<String>,
    pub renderable_surfaces: Vec<RenderableSurface>,
    renderable_surface_indices: HashMap<u32, usize>,
    locality_metrics: Cell<SurfaceLocalityMetrics>,
    active_scene_view: ActiveSceneView,
    scene_work_index: SceneWorkIndex,
    pub(in crate::compositor) tiled_layout: TiledLayoutManager,
    pub(in crate::compositor) tiled_resize_session: Option<TiledResizeSession>,
    pub(in crate::compositor) pending_tiled_resize: Option<PendingTiledResize>,
    pub(in crate::compositor) layout_generation: LayoutGeneration,
    layout_batch_depth: u8,
    layout_batch_scene_effect: bool,
    tiled_layout_dirty: HashSet<WorkspaceLocation>,
    tiled_floating_restores: HashMap<WindowId, WindowGeometry>,
    next_surface_id: u32,
    buffer_ids: BufferIdAllocator,
    surface_resources: HashMap<u32, wl_surface::WlSurface>,
    fifo_resources: HashMap<u32, wp_fifo_v1::WpFifoV1>,
    commit_timer_resources: HashMap<u32, wp_commit_timer_v1::WpCommitTimerV1>,
    tearing_control_resources: HashMap<u32, wp_tearing_control_v1::WpTearingControlV1>,
    content_type_resources: HashMap<u32, wp_content_type_v1::WpContentTypeV1>,
    active_fifo_barriers: HashMap<u32, ActiveFifoBarrier>,
    active_commit_timing_targets:
        HashMap<u32, Vec<(u64, SurfaceCommitSequence, CommitTimingReadiness)>>,
    next_fifo_barrier_generation: u64,
    surface_pacing_readiness_generation: u64,
    surface_pacing_serviced_generation: u64,
    surface_pacing_deadline_cache: Cell<Option<u64>>,
    surface_pacing_deadline_cache_valid: Cell<bool>,
    surface_pacing_deadline_recomputations: Cell<u64>,
    commit_timing_planning_pending: bool,
    commit_timing_planning_generation: u64,
    commit_timing_planning_signature: u64,
    surface_pacing_metrics: SurfacePacingMetrics,
    output_resources: Vec<wl_output::WlOutput>,
    workspace_protocol: WorkspaceProtocolState,
    fractional_scale_resources: HashMap<u32, Vec<wp_fractional_scale_v1::WpFractionalScaleV1>>,
    keyboard_resources: Vec<wl_keyboard::WlKeyboard>,
    pointer_resources: Vec<wl_pointer::WlPointer>,
    relative_pointer_resources: Vec<RelativePointerResource>,
    relative_pointer_resources_generation: u64,
    locked_relative_recipient_cache: LockedRelativeRecipientCache,
    idle_inhibitor_resources: Vec<IdleInhibitorBinding>,
    idle_manager: IdleManager,
    output_size: OutputSize,
    output_scale: OutputScale,
    output_refresh: OutputRefreshRate,
    presentation_clock: PresentationClock,
    focused_surface: Option<wl_surface::WlSurface>,
    focused_window_id: Option<WindowId>,
    focus_generation: u64,
    keyboard_surface: Option<wl_surface::WlSurface>,
    shortcut_inhibition: ShortcutInhibitionRegistry,
    keyboard_modifiers: KeyboardModifierState,
    pressed_keys: HashSet<u32>,
    pointer_surface: Option<wl_surface::WlSurface>,
    pointer_constraint: PointerConstraintState,
    pointer_constraints: HashMap<u64, PointerConstraint>,
    pending_pointer_constraint_surface_states: HashMap<u32, CapturedPointerConstraintSurfaceState>,
    next_internal_pointer_constraint_id: u64,
    next_pointer_constraint_generation: u64,
    active_locked_pointer_routing: Option<ActiveLockedPointerRouting>,
    active_confined_pointer_routing: Option<ActiveConfinedPointerRouting>,
    relative_motion_debug: RelativeMotionDebugState,
    dispatch_epoch: u64,
    active_backend_constraint: Option<PointerConstraintBackendId>,
    pending_backend_constraint: Option<PointerConstraintBackendId>,
    pending_locked_pointer_reveal: Option<PendingLockedPointerReveal>,
    pending_pointer_constraint_backend_requests: Vec<PointerConstraintBackendRequest>,
    cursor_visibility: CursorVisibilityState,
    pointer_entered_surfaces: Vec<(wl_pointer::WlPointer, wl_surface::WlSurface)>,
    pointer_enter_serials: Vec<PointerEnterSerial>,
    surface_role_lifecycles: HashMap<u32, SurfaceRoleLifecycle>,
    surface_client_ids: HashMap<u32, ClientId>,
    pending_client_resource_exhaustions: Vec<u32>,
    pub(in crate::compositor) desktop_windows: HashMap<WindowId, DesktopWindow>,
    pub(in crate::compositor) window_by_root_surface: HashMap<u32, WindowId>,
    pub(in crate::compositor) window_by_x11_handle: HashMap<X11WindowHandle, WindowId>,
    pub(in crate::compositor) next_window_id: u64,
    pub(in crate::compositor) workspace_manager: WorkspaceManager,
    pub(in crate::compositor) window_stacking: Vec<WindowId>,
    pub(in crate::compositor) applied_override_redirect_stack: Option<(XwaylandGeneration, u64)>,
    xwayland_scene_batch: XwaylandSceneBatchState,
    pub(in crate::compositor) backend_commands: Vec<window_backend::WindowBackendCommand>,
    cursor_surface_ids: HashSet<u32>,
    focused_client_cursor: Option<ClientCursorChoice>,
    client_cursor_surfaces: HashMap<u32, RenderableSurface>,
    xwayland: XwaylandCompositorState,
    surface_damage_journals: HashMap<u32, SurfaceDamageJournal>,
    // Monotonic damage-accounting baseline. NoVisualChange may advance it
    // without asserting that a physical output presentation occurred.
    presented_surface_commits: HashMap<u32, SurfaceCommitCounter>,
    surface_presentation_generations: HashMap<u32, u64>,
    next_surface_presentation_generation: u64,
    surface_publications: HashMap<u32, SurfacePublicationState>,
    surface_placements: HashMap<u32, SurfacePlacement>,
    committed_subsurface_stacks: HashMap<u32, Vec<u32>>,
    pending_subsurface_stacks: HashMap<u32, Vec<u32>>,
    subsurface_transactions: SubsurfaceTransactionState,
    subsurface_transaction_metrics: SubsurfaceTransactionMetrics,
    current_surface_buffers: HashMap<u32, CurrentSurfaceBuffer>,
    surface_window_geometries: HashMap<u32, XdgWindowGeometry>,
    pending_surface_window_geometries: HashMap<u32, XdgWindowGeometry>,
    surface_output_memberships: HashMap<u32, SurfaceOutputMembership>,
    preferred_output_transform: Option<wl_output::Transform>,
    xdg_surface_resources: HashMap<u32, xdg_surface::XdgSurface>,
    xdg_surface_wm_bases: HashMap<u32, xdg_wm_base::XdgWmBase>,
    xdg_surface_lifecycles: HashMap<u32, XdgSurfaceLifecycle>,
    xdg_decoration_states: HashMap<u32, WindowDecorationState>,
    xdg_decoration_resources: HashMap<u32, zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1>,
    decoration_button_capture: Option<DecorationButtonCapture>,
    decoration_button_hover: Option<(WindowId, DecorationButtonKind)>,
    decoration_titlebar_click_capture: Option<(WindowId, u32)>,
    decoration_last_titlebar_click: Option<(WindowId, Instant, f64, f64)>,
    decoration_theme: DecorationThemeSnapshot,
    decoration_theme_error: Option<String>,
    toplevel_surfaces: HashMap<u32, ToplevelSurface>,
    layer_surfaces: HashMap<u32, LayerSurfaceRole>,
    layer_surface_order: u64,
    exclusive_keyboard_layer_surface: Option<u32>,
    last_application_keyboard_focus: Option<wl_surface::WlSurface>,
    window_interaction: Option<WindowInteraction>,
    interaction_cursor_override: Option<InteractionCursorOverride>,
    fullscreen_presentation: Option<FullscreenPresentationState>,
    resize_configure_flows: HashMap<u32, ResizeConfigureFlow>,
    toplevel_visual_geometries: HashMap<u32, ToplevelVisualGeometry>,
    active_toplevel_resizes: HashMap<u32, ActiveToplevelResize>,
    pending_xwayland_visual_content: HashSet<u32>,
    next_window_interaction_id: u64,
    next_resize_interaction_id: u64,
    window_interaction_release_metrics: WindowInteractionReleaseMetrics,
    window_interaction_terminal_refresh_pending: bool,
    window_interaction_terminal_refresh_root_surface_id: Option<u32>,
    workspace_scene_transition_active: bool,
    window_interaction_release_debug: VecDeque<WindowInteractionReleaseDebugRecord>,
    next_resize_configure_sequence: u64,
    next_surface_commit_sequence: u64,
    next_surface_tree_transaction_id: u64,
    commit_debug: CommitDebugState,
    resize_flow_metrics: ResizeFlowMetrics,
    xdg_configure_serials: HashMap<u32, XdgConfigureSerialState>,
    last_pointer_x: f64,
    last_pointer_y: f64,
    last_pointer_motion_usec: Option<u64>,
    pending_window_interaction_pointer: Option<(WindowInteractionId, f64, f64)>,
    pending_floating_resize: Option<PendingFloatingResize>,
    last_relative_pointer_motion: Option<RelativePointerMotion>,
    last_pointer_press: Option<PointerPress>,
    held_pointer_buttons: Vec<PointerPress>,
    implicit_pointer_grab: Option<ImplicitPointerGrab>,
    recent_input_serials: Vec<InputSerial>,
    active_dmabuf_buffers: HashMap<u32, DmabufReleaseObligation>,
    pending_dmabuf_buffer_releases: Vec<DmabufReleaseObligation>,
    deferred_dmabuf_buffer_releases: Vec<DmabufReleaseObligation>,
    dmabuf_gpu_release_leases: HashMap<DmabufGpuReleaseLeaseId, DmabufGpuReleaseLease>,
    buffer_release_metrics: BufferReleaseMetrics,
    shm_buffer_lifetime_metrics: ShmBufferLifetimeMetrics,
    frame_callback_metrics: FrameCallbackMetrics,
    pending_explicit_sync_commits: Vec<PendingExplicitSyncCommit>,
    pending_surface_tree_transactions: Vec<PendingSurfaceTreeTransaction>,
    acquire_commit_ids: AcquireCommitIdAllocator,
    pending_acquire_watch_changes: Vec<AcquireWatchChange>,
    external_acquire_readiness: bool,
    pending_frame_callbacks: Vec<wl_callback::WlCallback>,
    visible_pending_frame_callbacks: Vec<wl_callback::WlCallback>,
    pending_frame_callback_surfaces: HashMap<ObjectId, u32>,
    pending_frame_callback_timing: HashMap<ObjectId, FrameCallbackTimingEvidence>,
    surface_frame_clock: HashMap<u32, SurfaceFrameClockState>,
    visible_pending_frame_callback_count: usize,
    pending_presentation_feedbacks: Vec<PendingPresentationFeedback>,
    visible_pending_presentation_feedbacks: Vec<PendingPresentationFeedback>,
    visible_pending_presentation_feedback_count: usize,
    frame_batches: HashMap<CompositorFrameBatchId, CompositorFrameBatch>,
    retired_frame_batches: HashMap<CompositorFrameBatchId, CompositorFrameBatch>,
    next_frame_batch_id: u64,
    next_legacy_output_frame_id: u64,
    legacy_prepared_frame_batch: Option<CompositorFrameBatchId>,
    legacy_submitted_frame_batch: Option<CompositorFrameBatchId>,
    pending_surface_presentation_feedbacks: HashMap<u32, Vec<PendingPresentationFeedback>>,
    frame_clock_start: Option<Instant>,
    next_configure_serial: u32,
    render_generation: u64,
    cursor_generation: u64,
    surface_tree_generation: Option<u64>,
    scene_render_generation: u64,
    pointer_hit_generation: u64,
    render_generation_cause: RenderGenerationCause,
    surface_origin_cache_generation: Option<u64>,
    surface_origin_cache: Vec<(i32, i32)>,
    visual_stack_groups_cache_generation: Option<u64>,
    visual_stack_groups_cache: Vec<VisualStackGroup>,
    pointer_scene_hit_cache: Option<PointerSceneHitCache>,
    pointer_hit_instrumentation_enabled: bool,
    pointer_hit_metrics: PointerInputMetrics,
    gpu_protocol_capabilities: GpuProtocolCapabilities,
    dmabuf_feedback: EglGlesDmabufFeedback,
    dmabuf_main_device: u64,
    dmabuf_main_device_path: Option<String>,
    dmabuf_scanout_capabilities: Option<DirectScanoutFeedbackCapabilities>,
    dmabuf_scanout_target_device_override: Option<u64>,
    syncobj_device: Option<DrmSyncobjDevice>,
    clipboard_bridge: Option<Box<dyn ClipboardBridge>>,
    selection_state: SelectionState,
    next_selection_source_key: u64,
    data_sources: HashMap<ObjectId, ClipboardDataSource>,
    data_devices: Vec<ClipboardDataDevice>,
    data_offers: HashMap<ObjectId, ClipboardDataOffer>,
    primary_sources: HashMap<ObjectId, PrimarySourceBinding>,
    primary_devices: Vec<PrimaryDeviceBinding>,
    primary_offers: HashMap<ObjectId, PrimaryOfferBinding>,
    data_control_sources: HashMap<ObjectId, DataControlSourceBinding>,
    data_control_devices: Vec<DataControlDeviceBinding>,
    data_control_offers: HashMap<ObjectId, DataControlOfferBinding>,
    active_drag: Option<ActiveDrag>,
    popup_surfaces: HashMap<u32, PopupSurface>,
    popup_grab_stack: Vec<u32>,
    popup_nodes: HashMap<u32, PopupNode>,
    popup_grab: Option<PopupGrab>,
    next_popup_grab_generation: u64,
    activation_tokens: HashMap<String, ActivationTokenState>,
    pending_activation_tokens: HashMap<u32, PendingActivationToken>,
    next_activation_token_serial: u64,
    pending_color_info: Vec<color::PendingColorInfo>,
    astrea_shortcut_registry: AstreaShortcutRegistry,
    astrea_toplevel_publisher: AstreaToplevelPublisher,
    astrea_toplevel_authorized_clients: HashSet<ClientId>,
    astrea_shell_authenticated_clients: HashSet<ClientId>,
    astrea_shell_client_pids: HashSet<u32>,
    astrea_shell_capability_verifier:
        Option<astrea_shell_capability::AstreaShellCapabilityVerifier>,
    typhon_socket_name: Option<String>,
    pending_process_launches: VecDeque<PendingProcessLaunch>,
    compliance_metrics: CoreComplianceMetrics,
}
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameBatchDiscardReason {
    RenderFailure,
    FatalOutputFailure,
    SuspendAbandonment,
    OutputDestroyed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SurfacePresentationKey {
    surface_id: u32,
    generation: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SurfaceDamageSettlement {
    Presented,
    NoVisualChange,
}
#[doc(hidden)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SurfaceDamagePresentation {
    sampled_commits: Vec<(SurfacePresentationKey, SurfaceCommitCounter)>,
}

impl SurfaceDamagePresentation {
    pub const fn is_empty(&self) -> bool {
        self.sampled_commits.is_empty()
    }

    pub fn contains_surface_id(&self, surface_id: u32) -> bool {
        self.sampled_commits
            .iter()
            .any(|(key, _)| key.surface_id == surface_id)
    }

    pub fn is_exclusive_surface_id(&self, surface_id: u32) -> bool {
        !self.sampled_commits.is_empty()
            && self
                .sampled_commits
                .iter()
                .all(|(key, _)| key.surface_id == surface_id)
    }
}

#[cfg(test)]
impl SurfaceDamagePresentation {
    pub(crate) fn sampled_surface_ids_for_test(&self) -> Vec<u32> {
        self.sampled_commits
            .iter()
            .map(|(key, _)| key.surface_id)
            .collect()
    }
}
#[derive(Debug, Clone)]
pub struct PendingProcessLaunch {
    pub argv: Vec<String>,
    pub request: astrea_launch_request_v1::AstreaLaunchRequestV1,
}
#[derive(Debug, Clone)]
struct AstreaShortcutRegistration {
    resource: astrea_shortcut_v1::AstreaShortcutV1,
    namespace: String,
    name: String,
}
#[derive(Debug, Clone)]
pub(in crate::compositor) struct ActiveLockedPointerRouting {
    constraint_id: u64,
    generation: u64,
    pointer: wl_pointer::WlPointer,
    surface: wl_surface::WlSurface,
    surface_x: f64,
    surface_y: f64,
    activation_anchor: OutputPosition,
}
#[derive(Debug, Clone)]
pub(in crate::compositor) struct ActiveConfinedPointerRouting {
    constraint_id: u64,
    generation: u64,
    pointer: wl_pointer::WlPointer,
    surface: wl_surface::WlSurface,
    region: OutputRegion,
}
#[derive(Debug, Clone)]
struct PendingLockedPointerReveal {
    backend_id: PointerConstraintBackendId,
    pointer: wl_pointer::WlPointer,
    surface: wl_surface::WlSurface,
    fallback_position: Option<OutputPosition>,
    backend_restore_settled: bool,
    backend_settled_dispatch_epoch: Option<u64>,
    client_warp_position: Option<OutputPosition>,
}
#[derive(Debug, Clone)]
struct ImplicitPointerGrab {
    surface: wl_surface::WlSurface,
    root_surface_id: u32,
}
#[derive(Debug, Default)]
struct RelativeMotionDebugState {
    pending_drop_reason: Option<String>,
    pending_drop_count: u32,
    last_drop_log: Option<Instant>,
    last_route_snapshot_log: Option<Instant>,
    dispatch_total: u64,
}
#[derive(Debug, Clone)]
struct RelativePointerResource {
    resource: zwp_relative_pointer_v1::ZwpRelativePointerV1,
    source_pointer: wl_pointer::WlPointer,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LockedRelativeRecipientCacheKey {
    resource_generation: u64,
    constraint_generation: u64,
    surface_id: u32,
    source_pointer_id: u32,
}
#[derive(Debug, Default)]
struct LockedRelativeRecipientCache {
    key: Option<LockedRelativeRecipientCacheKey>,
    recipients: Vec<RelativePointerResource>,
    frame_pointers: Vec<wl_pointer::WlPointer>,
    exact_source_pointer_count: usize,
    same_client_count: usize,
    same_seat_count: usize,
    stale_count: usize,
    cross_client_count: usize,
}
impl LockedRelativeRecipientCache {
    fn matches(&self, key: LockedRelativeRecipientCacheKey) -> bool {
        self.key == Some(key)
    }

    fn invalidate(&mut self) {
        self.key = None;
    }
}
#[derive(Debug, Clone)]
struct PointerConstraint {
    id: u64,
    generation: u64,
    mode: PointerConstraintMode,
    lifetime: PointerConstraintLifetime,
    surface: wl_surface::WlSurface,
    pointer: wl_pointer::WlPointer,
    locked_resource: Option<zwp_locked_pointer_v1::ZwpLockedPointerV1>,
    confined_resource: Option<zwp_confined_pointer_v1::ZwpConfinedPointerV1>,
    active: bool,
    backend_pending: bool,
    canceled_backend_activation: bool,
    protocol_resource_alive: bool,
    surface_constraint_pending: bool,
    lifecycle_removal_pending: bool,
    defunct: bool,
    committed: bool,
    committed_region: SurfaceInputRegion,
    committed_cursor_position_hint: Option<(f64, f64)>,
}
#[derive(Debug, Clone)]
pub(in crate::compositor) struct PointerConstraintRegistration {
    id: u64,
    mode: PointerConstraintMode,
    lifetime: PointerConstraintLifetime,
    surface: wl_surface::WlSurface,
    pointer: wl_pointer::WlPointer,
    locked_resource: Option<zwp_locked_pointer_v1::ZwpLockedPointerV1>,
    confined_resource: Option<zwp_confined_pointer_v1::ZwpConfinedPointerV1>,
    region: SurfaceInputRegion,
}
impl PointerConstraint {
    fn backend_id(&self) -> PointerConstraintBackendId {
        PointerConstraintBackendId {
            constraint_id: self.id,
            generation: self.generation,
        }
    }
}

#[derive(Debug, Clone)]
struct DataOfferData {
    target_client_id: ClientId,
    source_generation: u64,
    kind: DataOfferKind,
}

#[derive(Debug, Clone)]
struct ClipboardDataSource {
    source: wl_data_source::WlDataSource,
    selection_key: SelectionSourceKey,
    client_id: ClientId,
    mime_types: Vec<String>,
    use_state: DataSourceUse,
    actions: u32,
    actions_set: bool,
}
mod clipboard_state;
use clipboard_state::*;
use clipboard_state::{DataDeviceData, DataSourceData};
mod state;
use state::*;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod workspace_protocol_tests;
