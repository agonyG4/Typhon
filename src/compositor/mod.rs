use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::File,
    io,
    os::fd::{AsFd, OwnedFd},
    sync::{Arc, Mutex},
    time::Instant,
};

use crate::astrea_shortcuts::server::astrea_shortcut_v1;

pub use clipboard_bridge::{
    ClipboardBridge, ClipboardBridgeError, ClipboardBridgeEvent, HostClipboardOfferId,
    NoopClipboardBridge,
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

use crate::render_backend::buffer::{
    BufferId, BufferIdAllocator, BufferIdentity, BufferSize, DmabufBufferHandle,
    DmabufPlane as RenderDmabufPlane, DmabufPlaneDescriptor, DrmFormat, DrmModifier,
};
use crate::render_backend::egl_gles::EglGlesDmabufFeedback;
use crate::syncobj::DrmSyncobjDevice;
use crate::wayland_drm::server::wl_drm;

mod clipboard_bridge;
mod color;
mod dmabuf;
mod explicit_sync;
mod idle;
mod input;
mod interaction;
mod layer_shell;
mod output;
mod plan;
mod popup;
mod presentation;
mod protocols;
mod render;
mod runtime_files;
mod selection;
mod server;
mod shm;
mod state_data;
mod subsurface;
mod surface;
mod window_state;

use dmabuf::{
    DmabufBufferData, DmabufFeedbackData, DmabufParamsData, PendingDmabufPlane,
    default_dmabuf_main_device, send_dmabuf_feedback, send_dmabuf_format_modifiers,
    send_wl_drm_capabilities,
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
pub use idle::{IdleManager, IdleState};
use input::{
    InputSerial, KeyboardModifierState, PointerConstraintLifetime, send_keyboard_initial_state,
    send_pointer_frame_if_supported, wayland_event_time,
};
pub use input::{
    OutputPosition, OutputRect, OutputRegion, PointerConstraintBackendId,
    PointerConstraintBackendRequest, PointerConstraintMode, PointerConstraintState,
    PointerMotionSample, RelativePointerMotion,
};
pub use interaction::ResizeInteractionId;
use interaction::{
    PendingResizeConfigure, PointerPress, PointerTarget, ResizeAckDecision, ResizeCommitSnapshot,
    ResizeConfigureFlow, ResizeEdges, RootSurfaceHit, WindowFrameHit, WindowInteraction,
    WindowInteractionKind, interactive_resize_geometry, resize_drag_threshold_reached,
    resize_edges_for_window_point, resize_edges_from_xdg, window_frame_action_for_local_point,
};
use layer_shell::LayerSurfaceRole;
use output::{
    OutputRefreshRate, OutputScale, OutputSize, send_output_description,
    send_output_done_if_supported, send_output_mode, send_output_scale,
};
pub use plan::{
    ArchitectureLayer, CompositorArchitecture, CompositorPlan, InputProtocolCapabilities,
    ProtocolGlobal, RendererProtocolCapabilities, SelectionProtocolCapabilities,
    client_protocols_for_capabilities,
};
use popup::{
    PopupAnchorRect, PopupConstraintAdjustment, PopupEdges, PopupRect, XdgPositionerState,
    XdgWindowGeometry,
};
pub use presentation::{
    FramePresentation, PresentationClock, PresentationKind, PresentationTimestamp,
};
pub use render::{
    BufferAge, DesktopComposeRequest, DesktopFrameCopyKind, DesktopSceneRebuildKind,
    DesktopSceneRenderer, DesktopVisualState, NESTED_OUTPUT_BACKGROUND, RenderSceneElement,
    RenderSceneElementId, RenderSceneElementKind, ServerFrameColor, SurfaceRenderSpaceAssignment,
    SurfaceTargetRect, compose_nested_output, cursor_texture_pixels, cursor_texture_size,
    draw_wallpaper, output_scale_key, render_scene_elements_for_surfaces,
    scale_desktop_visual_state, scale_logical_coordinate, scale_logical_extent,
    server_frame_rects_by_surface, server_frame_rects_for_surface, surface_origin, surface_origins,
    surface_render_plan, surface_render_plan_with_clip, surface_render_space_assignments,
};
use runtime_files::{compositor_debug_surface_logging_enabled, unique_runtime_file_path};
pub use selection::{SelectionOfferRecord, SelectionState};
pub use server::{CompositorError, OwnCompositorServer};
use shm::{
    ShmBufferData, ShmPoolData, WL_SHM_FORMAT_ABGR8888, WL_SHM_FORMAT_ABGR2101010,
    WL_SHM_FORMAT_ARGB2101010, WL_SHM_FORMAT_XBGR8888, WL_SHM_FORMAT_XBGR2101010,
    WL_SHM_FORMAT_XRGB2101010,
};
use state_data::*;
use subsurface::{CachedSubsurfaceCommit, SubsurfaceSyncMode, SubsurfaceTransactionState};
pub use surface::{
    DamageSince, RenderableSurface, RenderableSurfaceDamage, RootPlacementMode,
    SurfaceCommitCounter, SurfaceCommitSequence, SurfaceDamageJournal, SurfaceDamageRect,
    SurfacePlacement,
};
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
    pub stale_interaction_commits_applied: u64,
    pub stale_commits_preserved_preview: u64,
    pub preview_ownership_transfers: u64,
    pub final_configures_sent: u64,
    pub resize_interactions_completed: u64,
    pub resize_interactions_canceled: u64,
    pub visual_geometry_resize_starts: u64,
    pub raw_pointer_resize_updates: u64,
    pub pending_resize_updates_replaced: u64,
    pub resize_updates_applied: u64,
    pub resize_updates_skipped_unchanged: u64,
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
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SubsurfaceTransactionMetrics {
    pub synchronized_child_commits_cached: u64,
    pub cached_commits_merged: u64,
    pub tree_transactions_prepared: u64,
    pub tree_transactions_published: u64,
    pub tree_transactions_waiting_on_acquire: u64,
    pub tree_transactions_superseded: u64,
    pub bufferless_tree_commits_merged: u64,
    pub metadata_only_nodes_merged: u64,
    pub attachments_replaced: u64,
    pub explicit_detaches: u64,
    pub acquire_dependencies_preserved: u64,
    pub acquire_dependencies_replaced: u64,
    pub ready_transactions_preserved_from_newer_unready: u64,
    pub callbacks_merged: u64,
    pub feedbacks_merged: u64,
    pub resize_snapshots_preserved: u64,
    pub resize_snapshots_replaced: u64,
    pub root_wide_supersessions: u64,
    pub waiting_transactions_published: u64,
    pub maximum_ready_slots_per_root: usize,
    pub maximum_waiting_slots_per_root: usize,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingInteractiveResizeUpdate {
    root_surface_id: u32,
    width: u32,
    height: u32,
    placement: SurfacePlacement,
    edges: ResizeEdges,
    interaction_id: ResizeInteractionId,
}

#[derive(Debug, Default, Clone, Copy)]
struct XdgConfigureSerialState {
    latest_sent: u32,
    latest_acked: u32,
}

#[derive(Debug)]
struct SurfaceTreeAcquireDependency {
    commit_id: AcquireCommitId,
    surface_id: u32,
    buffer_id: u32,
    acquire: ExplicitSyncPoint,
    state: PendingAcquireState,
}

#[derive(Debug)]
struct PendingSurfaceTreeTransaction {
    root_surface_id: u32,
    nodes: Vec<(u32, CachedSubsurfaceCommit)>,
    dependencies: Vec<SurfaceTreeAcquireDependency>,
    received_at: Instant,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct SurfacePublicationState {
    latest_received: SurfaceCommitSequence,
    latest_attachment_received: Option<SurfaceCommitSequence>,
    latest_published: Option<SurfaceCommitSequence>,
    latest_published_buffer_id: Option<BufferId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfacePublicationDecision {
    Publish,
    StaleAlreadyPublished,
    SupersededByNewerAttachment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum SurfacePublicationSource {
    Immediate,
    ExplicitSync,
    SurfaceTree,
    RemoveContent,
}

impl SurfacePublicationSource {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Immediate => "immediate",
            Self::ExplicitSync => "explicit_sync",
            Self::SurfaceTree => "surface_tree",
            Self::RemoveContent => "remove_content",
        }
    }
}

struct ReleasedSurfaceTreeState {
    callbacks: Vec<wl_callback::WlCallback>,
    resize_commit: Option<ResizeCommitSnapshot>,
}

#[derive(Debug, Default)]
struct SurfaceTreeMergeStats {
    incoming_nodes: usize,
    existing_nodes: usize,
    bufferless_nodes: usize,
    attachments_replaced: usize,
    explicit_detaches: usize,
    dependencies_preserved: usize,
    dependencies_replaced: usize,
    callbacks_merged: usize,
    feedbacks_merged: usize,
    resize_snapshots_preserved: usize,
    resize_snapshots_replaced: usize,
}

struct BufferlessSurfaceCommitState {
    commit_sequence: SurfaceCommitSequence,
    damage: Option<RenderableSurfaceDamage>,
    explicit_sync: Option<Arc<SyncobjSurfaceState>>,
    surface_size: Option<BufferSize>,
    buffer_scale: u32,
    resize_commit: Option<ResizeCommitSnapshot>,
    resize_capture_finalized: bool,
    window_geometry: Option<XdgWindowGeometry>,
}

impl PendingSurfaceTreeTransaction {
    fn is_ready(&self) -> bool {
        self.dependencies
            .iter()
            .all(|dependency| dependency.state == PendingAcquireState::Ready)
    }
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
    WindowMinimize,
    WindowRestore,
    WindowStack,
    OutputChange,
    CursorCommit,
    CursorMotion,
    CursorState,
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
            Self::WindowMinimize => "window_minimize",
            Self::WindowRestore => "window_restore",
            Self::WindowStack => "window_stack",
            Self::OutputChange => "output_change",
            Self::CursorCommit => "cursor_commit",
            Self::CursorMotion => "cursor_motion",
            Self::CursorState => "cursor_state",
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
}

#[derive(Debug, Default)]
pub struct CompositorState {
    pub accepted_clients: usize,
    pub xdg_toplevels: usize,
    pub xdg_popups: usize,
    pub last_app_id: Option<String>,
    pub renderable_surfaces: Vec<RenderableSurface>,
    next_surface_id: u32,
    buffer_ids: BufferIdAllocator,
    surface_resources: HashMap<u32, wl_surface::WlSurface>,
    output_resources: Vec<wl_output::WlOutput>,
    fractional_scale_resources: HashMap<u32, Vec<wp_fractional_scale_v1::WpFractionalScaleV1>>,
    keyboard_resources: Vec<wl_keyboard::WlKeyboard>,
    pointer_resources: Vec<wl_pointer::WlPointer>,
    relative_pointer_resources: Vec<RelativePointerResource>,
    idle_inhibitor_resources: Vec<zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1>,
    idle_manager: IdleManager,
    output_size: OutputSize,
    output_scale: OutputScale,
    output_refresh: OutputRefreshRate,
    presentation_clock: PresentationClock,
    focused_surface: Option<wl_surface::WlSurface>,
    keyboard_surface: Option<wl_surface::WlSurface>,
    keyboard_modifiers: KeyboardModifierState,
    pointer_surface: Option<wl_surface::WlSurface>,
    pointer_constraint: PointerConstraintState,
    pointer_constraints: HashMap<u64, PointerConstraint>,
    next_internal_pointer_constraint_id: u64,
    next_pointer_constraint_generation: u64,
    active_locked_pointer_routing: Option<ActiveLockedPointerRouting>,
    active_confined_pointer_routing: Option<ActiveConfinedPointerRouting>,
    relative_motion_debug: RelativeMotionDebugState,
    dispatch_epoch: u64,
    active_backend_constraint: Option<PointerConstraintBackendId>,
    pending_backend_constraint: Option<PointerConstraintBackendId>,
    pending_locked_activation_anchors: HashMap<PointerConstraintBackendId, OutputPosition>,
    pending_locked_pointer_reveal: Option<PendingLockedPointerReveal>,
    pending_pointer_constraint_backend_requests: Vec<PointerConstraintBackendRequest>,
    cursor_visibility: CursorVisibilityState,
    pointer_entered_surfaces: Vec<(wl_pointer::WlPointer, wl_surface::WlSurface)>,
    pointer_enter_serials: Vec<PointerEnterSerial>,
    cursor_surface_ids: HashSet<u32>,
    active_client_cursor: Option<ActiveClientCursor>,
    client_cursor_surfaces: HashMap<u32, RenderableSurface>,
    surface_damage_journals: HashMap<u32, SurfaceDamageJournal>,
    presented_surface_commits: HashMap<u32, SurfaceCommitCounter>,
    surface_publications: HashMap<u32, SurfacePublicationState>,
    surface_placements: HashMap<u32, SurfacePlacement>,
    committed_subsurface_stacks: HashMap<u32, Vec<u32>>,
    pending_subsurface_stacks: HashMap<u32, Vec<u32>>,
    subsurface_transactions: SubsurfaceTransactionState,
    subsurface_transaction_metrics: SubsurfaceTransactionMetrics,
    current_surface_buffers: HashMap<u32, PendingSurfaceBuffer>,
    surface_window_geometries: HashMap<u32, XdgWindowGeometry>,
    pending_surface_window_geometries: HashMap<u32, XdgWindowGeometry>,
    surface_entered_outputs: HashSet<(u32, u32)>,
    toplevel_surfaces: HashMap<u32, ToplevelSurface>,
    layer_surfaces: HashMap<u32, LayerSurfaceRole>,
    layer_surface_order: u64,
    exclusive_keyboard_layer_surface: Option<u32>,
    last_application_keyboard_focus: Option<wl_surface::WlSurface>,
    configured_xdg_surfaces: HashSet<u32>,
    window_interaction: Option<WindowInteraction>,
    pending_interactive_resize_update: Option<PendingInteractiveResizeUpdate>,
    resize_configure_flows: HashMap<u32, ResizeConfigureFlow>,
    toplevel_visual_geometries: HashMap<u32, ToplevelVisualGeometry>,
    active_toplevel_resizes: HashMap<u32, ActiveToplevelResize>,
    next_resize_interaction_id: u64,
    next_resize_configure_sequence: u64,
    next_surface_commit_sequence: u64,
    resize_flow_metrics: ResizeFlowMetrics,
    xdg_configure_serials: HashMap<u32, XdgConfigureSerialState>,
    last_pointer_x: f64,
    last_pointer_y: f64,
    last_pointer_motion_usec: Option<u64>,
    last_relative_pointer_motion: Option<RelativePointerMotion>,
    last_pointer_press: Option<PointerPress>,
    held_pointer_buttons: Vec<PointerPress>,
    implicit_pointer_grab: Option<ImplicitPointerGrab>,
    recent_input_serials: Vec<InputSerial>,
    active_dmabuf_buffers: HashMap<u32, SurfaceBufferRelease>,
    pending_buffer_releases: Vec<wl_buffer::WlBuffer>,
    pending_dmabuf_buffer_releases: Vec<SurfaceBufferRelease>,
    deferred_dmabuf_buffer_releases: Vec<SurfaceBufferRelease>,
    pending_explicit_sync_commits: Vec<PendingExplicitSyncCommit>,
    pending_surface_tree_transactions: Vec<PendingSurfaceTreeTransaction>,
    acquire_commit_ids: AcquireCommitIdAllocator,
    pending_acquire_watch_changes: Vec<AcquireWatchChange>,
    external_acquire_readiness: bool,
    pending_frame_callbacks: Vec<wl_callback::WlCallback>,
    pending_presentation_feedbacks: Vec<PendingPresentationFeedback>,
    pending_surface_presentation_feedbacks: HashMap<u32, Vec<PendingPresentationFeedback>>,
    frame_clock_start: Option<Instant>,
    next_configure_serial: u32,
    render_generation: u64,
    surface_tree_generation: Option<u64>,
    scene_render_generation: u64,
    render_generation_cause: RenderGenerationCause,
    surface_origin_cache_generation: Option<u64>,
    surface_origin_cache: Vec<(i32, i32)>,
    dmabuf_feedback: EglGlesDmabufFeedback,
    dmabuf_main_device: u64,
    dmabuf_main_device_path: Option<String>,
    syncobj_device: Option<DrmSyncobjDevice>,
    clipboard_bridge: Option<Box<dyn ClipboardBridge>>,
    selection_state: SelectionState,
    data_sources: HashMap<ObjectId, ClipboardDataSource>,
    data_devices: Vec<ClipboardDataDevice>,
    data_offers: HashMap<ObjectId, ClipboardDataOffer>,
    active_clipboard: Option<ActiveClipboard>,
    next_clipboard_generation: u64,
    popup_surfaces: HashMap<u32, PopupSurface>,
    popup_grab_stack: Vec<u32>,
    popup_nodes: HashMap<u32, PopupNode>,
    popup_grab: Option<PopupGrab>,
    next_popup_grab_generation: u64,
    activation_tokens: HashMap<String, ActivationTokenState>,
    pending_activation_tokens: HashMap<u32, PendingActivationToken>,
    next_activation_token_serial: u64,
    pending_color_info: Vec<color::PendingColorInfo>,
    astrea_shortcuts: Vec<AstreaShortcutRegistration>,
    astrea_shell_client_pids: HashSet<u32>,
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
    created_dispatch_epoch: u64,
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
    defunct: bool,
    pending_region: SurfaceInputRegion,
    committed_region: SurfaceInputRegion,
    pending_cursor_position_hint: Option<(f64, f64)>,
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
struct DataSourceData {
    client_id: ClientId,
}

#[derive(Debug, Clone)]
struct DataDeviceData {
    client_id: ClientId,
    seat_id: ObjectId,
}

#[derive(Debug, Clone)]
struct DataOfferData {
    target_client_id: ClientId,
    source_generation: u64,
}

#[derive(Debug, Clone)]
struct ClipboardDataSource {
    source: wl_data_source::WlDataSource,
    client_id: ClientId,
    mime_types: Vec<String>,
}

#[derive(Debug, Clone)]
struct ClipboardDataDevice {
    device: wl_data_device::WlDataDevice,
    client_id: ClientId,
    seat_id: ObjectId,
}

#[derive(Debug, Clone)]
struct ClipboardDataOffer {
    offer: wl_data_offer::WlDataOffer,
    target_client_id: ClientId,
    source_generation: u64,
    mime_types: Vec<String>,
}

#[derive(Debug, Clone)]
struct ActiveClipboard {
    generation: u64,
    source: ClipboardSourceBackend,
    mime_types: Vec<String>,
}

#[derive(Debug, Clone)]
enum ClipboardSourceBackend {
    InternalWayland {
        source: wl_data_source::WlDataSource,
        client_id: ClientId,
    },
    HostBridge {
        offer_id: HostClipboardOfferId,
    },
}

mod state;
use state::*;

#[cfg(test)]
mod tests;
