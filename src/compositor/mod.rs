use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io,
    os::fd::{AsFd, OwnedFd},
    sync::{Arc, Mutex},
    time::Instant,
};

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
    decoration::zv1::server::{zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1},
    shell::server::{xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base},
};
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
    BufferIdAllocator, BufferIdentity, BufferSize, DmabufBufferHandle,
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
mod output;
mod plan;
mod popup;
mod presentation;
mod protocols;
mod render;
mod runtime_files;
mod selection;
mod server;
mod shell;
mod shm;
mod state_data;
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
    AcquireCommitIdAllocator, PendingAcquireState, PendingExplicitSyncCommit,
    PendingPresentationFeedback, SYNCOBJ_MANAGER_ERROR_INVALID_TIMELINE,
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
#[cfg(test)]
use interaction::PendingResizeCommit;
use interaction::{
    PendingResizeConfigure, PointerPress, PointerTarget, ResizeAckDecision, ResizeCommitSnapshot,
    ResizeConfigureFlow, ResizeEdges, RootSurfaceHit, WindowFrameHit, WindowInteraction,
    WindowInteractionKind, interactive_resize_geometry, resize_drag_threshold_reached,
    resize_edges_for_window_point, resize_edges_from_xdg, window_frame_action_for_local_point,
};
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
    RenderSceneElementId, RenderSceneElementKind, ServerFrameColor, SurfaceTargetRect,
    compose_nested_output, cursor_texture_pixels, cursor_texture_size, draw_wallpaper,
    output_scale_key, render_scene_elements_for_surfaces, scale_desktop_visual_state,
    scale_logical_coordinate, scale_logical_extent, server_frame_rects_by_surface,
    server_frame_rects_for_surface, surface_origin, surface_origins, surface_render_plan,
};
use runtime_files::{compositor_debug_surface_logging_enabled, unique_runtime_file_path};
pub use selection::{SelectionOfferRecord, SelectionState};
pub use server::{CompositorError, OwnCompositorServer};
pub use shell::{
    ShellDockItem, ShellLaunchSuggestion, ShellOverlayBounds, ShellOverlayImage,
    ShellOverlayRenderer, ShellOverlayState, ShellTopbarModel, SpotlightModel, dock_item_at,
    launcher_suggestions,
};
use shm::{
    ShmBufferData, ShmPoolData, WL_SHM_FORMAT_ABGR8888, WL_SHM_FORMAT_ABGR2101010,
    WL_SHM_FORMAT_ARGB2101010, WL_SHM_FORMAT_XBGR8888, WL_SHM_FORMAT_XBGR2101010,
    WL_SHM_FORMAT_XRGB2101010,
};
use state_data::*;
pub use surface::{
    RenderableSurface, RenderableSurfaceDamage, ResizePreview, SurfaceDamageRect, SurfacePlacement,
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
    pub commits_captured: u64,
    pub commits_delayed_by_explicit_sync: u64,
    pub preview_activations: u64,
    pub preview_completions: u64,
    pub max_preview_age_ms: u64,
    pub max_in_flight_configures: usize,
    pub max_pending_explicit_sync_commits: usize,
}

#[derive(Debug, Clone, Copy)]
struct ResizePreviewMetadata {
    flow_sequence: u64,
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
    surface_placements: HashMap<u32, SurfacePlacement>,
    committed_subsurface_stacks: HashMap<u32, Vec<u32>>,
    pending_subsurface_stacks: HashMap<u32, Vec<u32>>,
    current_surface_buffers: HashMap<u32, PendingSurfaceBuffer>,
    surface_window_geometries: HashMap<u32, XdgWindowGeometry>,
    pending_window_geometry_commits: HashSet<u32>,
    surface_entered_outputs: HashSet<(u32, u32)>,
    toplevel_surfaces: HashMap<u32, ToplevelSurface>,
    configured_xdg_surfaces: HashSet<u32>,
    window_interaction: Option<WindowInteraction>,
    resize_configure_flows: HashMap<u32, ResizeConfigureFlow>,
    next_resize_configure_sequence: u64,
    next_surface_commit_sequence: u64,
    resize_flow_metrics: ResizeFlowMetrics,
    resize_preview_metadata: HashMap<u32, ResizePreviewMetadata>,
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
    acquire_commit_ids: AcquireCommitIdAllocator,
    pending_acquire_watch_changes: Vec<AcquireWatchChange>,
    external_acquire_readiness: bool,
    pending_frame_callbacks: Vec<wl_callback::WlCallback>,
    pending_presentation_feedbacks: Vec<PendingPresentationFeedback>,
    frame_clock_start: Option<Instant>,
    next_configure_serial: u32,
    render_generation: u64,
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
    pending_color_info: Vec<color::PendingColorInfo>,
}

#[derive(Debug, Clone)]
struct ActiveLockedPointerRouting {
    constraint_id: u64,
    generation: u64,
    pointer: wl_pointer::WlPointer,
    surface: wl_surface::WlSurface,
    surface_x: f64,
    surface_y: f64,
    activation_anchor: OutputPosition,
}

#[derive(Debug, Clone)]
struct ActiveConfinedPointerRouting {
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
struct PointerConstraintRegistration {
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

fn coalesce_output_row_rects(rects: Vec<OutputRect>) -> Vec<OutputRect> {
    let mut coalesced: Vec<OutputRect> = Vec::new();
    for rect in rects {
        if let Some(last) = coalesced.last_mut()
            && last.x == rect.x
            && last.width == rect.width
            && (last.y + last.height) == rect.y
        {
            last.height += rect.height;
            continue;
        }
        coalesced.push(rect);
    }
    coalesced
}

#[derive(Debug, Clone)]
struct CursorVisibilityState {
    client_hidden_pointer: Option<wl_pointer::WlPointer>,
    client_cursor_pointer: Option<wl_pointer::WlPointer>,
    lock_hidden_constraint_id: Option<u64>,
    visible: bool,
}

impl Default for CursorVisibilityState {
    fn default() -> Self {
        Self {
            client_hidden_pointer: None,
            client_cursor_pointer: None,
            lock_hidden_constraint_id: None,
            visible: true,
        }
    }
}

impl CursorVisibilityState {
    fn desired_visible(&self) -> bool {
        self.client_hidden_pointer.is_none()
            && self.client_cursor_pointer.is_none()
            && self.lock_hidden_constraint_id.is_none()
    }
}

#[derive(Debug, Clone)]
struct PointerEnterSerial {
    pointer: wl_pointer::WlPointer,
    surface: wl_surface::WlSurface,
    serial: u32,
}

#[derive(Debug, Clone)]
struct ActiveClientCursor {
    pointer: wl_pointer::WlPointer,
    surface_id: u32,
    hotspot_x: i32,
    hotspot_y: i32,
}

fn pointer_debug_log(message: impl AsRef<str>) {
    if std::env::var_os("TYPHON_POINTER_DEBUG").is_some() {
        eprintln!("typhon pointer: {}", message.as_ref());
    }
}

impl RelativeMotionDebugState {
    fn note_dispatch(&mut self, message: String) {
        self.dispatch_total = self.dispatch_total.saturating_add(1);
        pointer_debug_log(message);
    }

    fn note_drop(&mut self, reason: impl Into<String>) {
        self.pending_drop_count = self.pending_drop_count.saturating_add(1);
        self.pending_drop_reason = Some(reason.into());
        self.flush_drops(false);
    }

    fn should_log_route_snapshot(&mut self) -> bool {
        if std::env::var_os("TYPHON_POINTER_DEBUG").is_none() {
            return false;
        }
        let now = Instant::now();
        let should_log = self
            .last_route_snapshot_log
            .is_none_or(|last| now.duration_since(last) >= std::time::Duration::from_millis(500));
        if should_log {
            self.last_route_snapshot_log = Some(now);
        }
        should_log
    }

    fn flush_drops(&mut self, force: bool) {
        let Some(reason) = self.pending_drop_reason.take() else {
            return;
        };
        let count = self.pending_drop_count;
        self.pending_drop_count = 0;
        let now = Instant::now();
        let should_log = force
            || self.last_drop_log.is_none_or(|last| {
                now.duration_since(last) >= std::time::Duration::from_millis(500)
            });
        if !should_log {
            self.pending_drop_reason = Some(reason);
            self.pending_drop_count = count;
            return;
        }
        self.last_drop_log = Some(now);
        if count > 1 {
            pointer_debug_log(format!("relative motion drop reason ({count}x): {reason}"));
        } else {
            pointer_debug_log(format!("relative motion drop reason: {reason}"));
        }
    }
}

fn wayland_resource_client_label(resource: &impl Resource) -> String {
    resource
        .client()
        .map(|client| format!("{:?}", client.id()))
        .unwrap_or_else(|| "unknown".to_string())
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

impl CompositorState {
    fn new(syncobj_device: Option<DrmSyncobjDevice>) -> Self {
        let default_dmabuf_device = default_dmabuf_main_device();
        Self {
            frame_clock_start: Some(Instant::now()),
            dmabuf_feedback: EglGlesDmabufFeedback::default(),
            dmabuf_main_device: default_dmabuf_device
                .as_ref()
                .map(|device| device.rdev)
                .unwrap_or(0),
            dmabuf_main_device_path: default_dmabuf_device.map(|device| device.path),
            syncobj_device,
            clipboard_bridge: Some(Box::new(NoopClipboardBridge)),
            ..Self::default()
        }
    }

    fn allocate_buffer_identity(&mut self) -> Option<BufferIdentity> {
        self.buffer_ids.allocate()
    }

    fn next_render_generation_value(&self) -> u64 {
        self.render_generation.saturating_add(1)
    }

    fn set_render_generation(&mut self, generation: u64, cause: RenderGenerationCause) {
        self.render_generation = generation;
        self.render_generation_cause = cause;
        if !matches!(
            cause,
            RenderGenerationCause::CursorCommit
                | RenderGenerationCause::CursorMotion
                | RenderGenerationCause::CursorState
        ) {
            self.scene_render_generation = generation;
        }
    }

    fn advance_render_generation(&mut self, cause: RenderGenerationCause) -> u64 {
        let generation = self.next_render_generation_value();
        self.set_render_generation(generation, cause);
        self.update_all_active_confined_pointer_regions(cause.as_str());
        generation
    }

    fn render_generation_cause(&self) -> RenderGenerationCause {
        self.render_generation_cause
    }

    fn set_dmabuf_feedback(
        &mut self,
        feedback: EglGlesDmabufFeedback,
        main_device: Option<u64>,
        main_device_path: Option<String>,
    ) {
        self.dmabuf_feedback = feedback;
        self.dmabuf_main_device = main_device.filter(|device| *device != 0).unwrap_or(0);
        self.dmabuf_main_device_path = main_device_path.filter(|path| !path.is_empty());
    }

    fn set_output_size(&mut self, width: u32, height: u32) -> bool {
        let output_size = OutputSize::new(width, height);
        if self.output_size == output_size {
            return false;
        }

        self.output_size = output_size;
        self.send_output_mode_to_bound_outputs();
        self.reconfigure_stateful_windows_for_output_size();
        true
    }

    fn set_output_scale_factor(&mut self, scale_factor: f64) -> bool {
        let output_scale = OutputScale::from_factor(scale_factor);
        if self.output_scale == output_scale {
            return false;
        }

        self.output_scale = output_scale;
        self.send_output_scale_to_bound_outputs();
        self.send_fractional_scale_to_bound_surfaces();
        self.advance_render_generation(RenderGenerationCause::OutputChange);
        true
    }

    fn set_output_refresh_hz(&mut self, refresh_hz: u32) -> bool {
        let output_refresh = OutputRefreshRate::from_hz(refresh_hz);
        if self.output_refresh == output_refresh {
            return false;
        }

        self.output_refresh = output_refresh;
        self.send_output_mode_to_bound_outputs();
        true
    }

    pub fn note_xdg_toplevel_created(&mut self, app_id: impl Into<String>) {
        self.xdg_toplevels += 1;
        self.last_app_id = Some(app_id.into());
    }

    fn note_xdg_popup_created(&mut self) {
        self.xdg_popups += 1;
    }

    fn next_configure_serial(&mut self) -> u32 {
        self.next_configure_serial = self.next_configure_serial.saturating_add(1);
        self.next_configure_serial
    }

    fn allocate_surface_id(&mut self) -> u32 {
        self.next_surface_id = self.next_surface_id.saturating_add(1).max(1);
        self.next_surface_id
    }

    fn frame_callback_time_ms(&mut self) -> u32 {
        let start = self.frame_clock_start.get_or_insert_with(Instant::now);
        start.elapsed().as_millis() as u32
    }

    fn focus_surface(&mut self, surface: wl_surface::WlSurface) {
        self.set_desktop_focus(surface, "focus");
    }

    fn set_desktop_focus(&mut self, surface: wl_surface::WlSurface, reason: &'static str) {
        let old_surface_id = self.focused_surface.as_ref().map(compositor_surface_id);
        let new_surface_id = compositor_surface_id(&surface);
        let changed = !self
            .focused_surface
            .as_ref()
            .is_some_and(|current| same_surface_resource(current, &surface));
        if changed {
            pointer_debug_log(format!(
                "focus change reason={} old={:?} new={}",
                reason, old_surface_id, new_surface_id
            ));
        }
        self.focused_surface = Some(surface.clone());
        self.ensure_keyboard_focus(&surface);
        self.apply_pending_pointer_constraint_state_for_surface(new_surface_id);
    }

    fn focused_client_id(&self) -> Option<ClientId> {
        self.focused_surface
            .as_ref()
            .and_then(Resource::client)
            .map(|client| client.id())
    }

    fn client_has_focus(&self, client_id: &ClientId) -> bool {
        self.focused_client_id()
            .as_ref()
            .is_some_and(|focused_client_id| focused_client_id == client_id)
    }

    fn remember_input_serial(&mut self, serial: u32, surface: wl_surface::WlSurface) {
        self.recent_input_serials
            .retain(|input| input.serial != serial);
        self.recent_input_serials
            .push(InputSerial { serial, surface });
        const MAX_RECENT_INPUT_SERIALS: usize = 16;
        let excess = self
            .recent_input_serials
            .len()
            .saturating_sub(MAX_RECENT_INPUT_SERIALS);
        if excess > 0 {
            self.recent_input_serials.drain(0..excess);
        }
    }

    fn has_recent_input_serial_for_surface(
        &self,
        serial: u32,
        surface: &wl_surface::WlSurface,
    ) -> bool {
        self.recent_input_serials
            .iter()
            .any(|input| input.serial == serial && input.surface.id().same_client_as(&surface.id()))
    }

    fn client_has_recent_input_serial(&self, client_id: &ClientId, serial: u32) -> bool {
        self.recent_input_serials.iter().any(|input| {
            input.serial == serial
                && input
                    .surface
                    .client()
                    .is_some_and(|client| client.id() == *client_id)
        })
    }

    fn register_data_source(&mut self, source: wl_data_source::WlDataSource, client_id: ClientId) {
        self.selection_state.begin_source(source.id().protocol_id());
        self.data_sources.insert(
            source.id(),
            ClipboardDataSource {
                source,
                client_id,
                mime_types: Vec::new(),
            },
        );
    }

    fn offer_data_source_mime_type(
        &mut self,
        source: &wl_data_source::WlDataSource,
        mime_type: String,
    ) {
        self.selection_state
            .offer_source_mime_type(source.id().protocol_id(), mime_type.clone());
        let Some(binding) = self.data_sources.get_mut(&source.id()) else {
            return;
        };
        if mime_type.is_empty()
            || mime_type.len() > 4096
            || binding.mime_types.len() >= 128
            || binding
                .mime_types
                .iter()
                .any(|existing| existing == &mime_type)
        {
            return;
        }
        binding.mime_types.push(mime_type);
    }

    fn remove_data_source(&mut self, source: &wl_data_source::WlDataSource) {
        self.data_sources.remove(&source.id());
        self.selection_state
            .remove_source(source.id().protocol_id());
        if self
            .active_clipboard
            .as_ref()
            .is_some_and(|selection| match &selection.source {
                ClipboardSourceBackend::InternalWayland {
                    source: active_source,
                    ..
                } => same_wayland_resource(active_source, source),
                ClipboardSourceBackend::HostBridge { .. } => false,
            })
        {
            self.active_clipboard = None;
            self.next_clipboard_generation = self.next_clipboard_generation.saturating_add(1);
            if let Some(bridge) = self.clipboard_bridge.as_mut() {
                let _ = bridge.clear_internal_selection();
            }
            self.data_offers.clear();
            self.publish_clipboard_to_focused_client();
        }
    }

    fn register_data_device(
        &mut self,
        device: wl_data_device::WlDataDevice,
        client_id: ClientId,
        seat_id: ObjectId,
    ) {
        self.data_devices
            .retain(|binding| binding.device.is_alive());
        self.data_devices.push(ClipboardDataDevice {
            device: device.clone(),
            client_id: client_id.clone(),
            seat_id,
        });
        if self.client_has_focus(&client_id) {
            self.publish_clipboard_to_data_device(&device);
        }
    }

    fn remove_data_device(&mut self, device: &wl_data_device::WlDataDevice) {
        self.data_devices
            .retain(|binding| !same_wayland_resource(&binding.device, device));
        self.data_offers.retain(|_, offer| {
            offer.offer.is_alive() && !offer.offer.id().same_client_as(&device.id())
        });
    }

    fn set_clipboard_selection(
        &mut self,
        client_id: &ClientId,
        source: Option<wl_data_source::WlDataSource>,
        serial: u32,
    ) -> bool {
        if !self.client_has_focus(client_id)
            || !self.client_has_recent_input_serial(client_id, serial)
        {
            return false;
        }

        let Some(source) = source else {
            self.active_clipboard = None;
            self.selection_state.clear_clipboard_selection();
            self.next_clipboard_generation = self.next_clipboard_generation.saturating_add(1);
            if let Some(bridge) = self.clipboard_bridge.as_mut() {
                let _ = bridge.clear_internal_selection();
            }
            self.data_offers.clear();
            self.publish_clipboard_to_focused_client();
            return true;
        };

        let Some(binding) = self.data_sources.get(&source.id()).cloned() else {
            return false;
        };
        if binding.client_id != *client_id || !source.is_alive() || binding.mime_types.is_empty() {
            return false;
        }

        if let Some(previous) = self.active_clipboard.as_ref()
            && let ClipboardSourceBackend::InternalWayland {
                source: previous_source,
                ..
            } = &previous.source
            && !same_wayland_resource(previous_source, &source)
            && previous_source.is_alive()
        {
            previous_source.cancelled();
        }

        self.next_clipboard_generation = self.next_clipboard_generation.saturating_add(1);
        let generation = self.next_clipboard_generation;
        self.selection_state
            .set_clipboard_selection_from_source(source.id().protocol_id());
        self.active_clipboard = Some(ActiveClipboard {
            generation,
            source: ClipboardSourceBackend::InternalWayland {
                source: binding.source,
                client_id: binding.client_id,
            },
            mime_types: binding.mime_types.clone(),
        });
        if let Some(bridge) = self.clipboard_bridge.as_mut() {
            let _ = bridge.publish_internal_selection(generation, binding.mime_types);
        }
        self.data_offers.clear();
        self.publish_clipboard_to_focused_client();
        true
    }

    fn install_host_clipboard_selection(
        &mut self,
        offer_id: HostClipboardOfferId,
        mime_types: Vec<String>,
    ) {
        let mime_types = normalize_selection_mime_types(mime_types);
        if mime_types.is_empty() {
            self.clear_host_clipboard_selection();
            return;
        }
        self.next_clipboard_generation = self.next_clipboard_generation.saturating_add(1);
        self.active_clipboard = Some(ActiveClipboard {
            generation: self.next_clipboard_generation,
            source: ClipboardSourceBackend::HostBridge { offer_id },
            mime_types,
        });
        self.data_offers.clear();
        self.publish_clipboard_to_focused_client();
    }

    fn clear_host_clipboard_selection(&mut self) {
        self.next_clipboard_generation = self.next_clipboard_generation.saturating_add(1);
        if self.active_clipboard.as_ref().is_some_and(|selection| {
            matches!(selection.source, ClipboardSourceBackend::HostBridge { .. })
        }) {
            self.active_clipboard = None;
            self.data_offers.clear();
            self.publish_clipboard_to_focused_client();
        }
    }

    fn poll_clipboard_bridge(&mut self) {
        let Some(bridge) = self.clipboard_bridge.as_mut() else {
            return;
        };
        let events = bridge.poll_events();
        for event in events {
            match event {
                ClipboardBridgeEvent::HostSelectionChanged {
                    offer_id,
                    mime_types,
                } => self.install_host_clipboard_selection(offer_id, mime_types),
                ClipboardBridgeEvent::HostSelectionCleared => self.clear_host_clipboard_selection(),
            }
        }
    }

    fn publish_clipboard_to_focused_client(&mut self) {
        let Some(client_id) = self.focused_client_id() else {
            return;
        };
        let devices = self
            .data_devices
            .iter()
            .filter(|binding| {
                binding.client_id == client_id
                    && binding.device.is_alive()
                    && binding.seat_id.interface().name == "wl_seat"
            })
            .map(|binding| binding.device.clone())
            .collect::<Vec<_>>();
        for device in devices {
            self.publish_clipboard_to_data_device(&device);
        }
    }

    fn publish_clipboard_to_data_device(&mut self, device: &wl_data_device::WlDataDevice) {
        if !device.is_alive() {
            return;
        }
        let Some(selection) = self.active_clipboard.clone() else {
            let _ = device.send_event(wl_data_device::Event::Selection { id: None });
            return;
        };
        if selection.mime_types.is_empty() {
            let _ = device.send_event(wl_data_device::Event::Selection { id: None });
            return;
        }
        let Some(client) = device.client() else {
            return;
        };
        let Some(handle) = device.handle().upgrade() else {
            return;
        };
        let display = DisplayHandle::from(handle);
        let Ok(offer) = client
            .create_resource::<wl_data_offer::WlDataOffer, DataOfferData, CompositorState>(
                &display,
                device.version().min(3),
                DataOfferData {
                    target_client_id: client.id(),
                    source_generation: selection.generation,
                },
            )
        else {
            return;
        };

        self.data_offers.insert(
            offer.id(),
            ClipboardDataOffer {
                offer: offer.clone(),
                target_client_id: client.id(),
                source_generation: selection.generation,
                mime_types: selection.mime_types.clone(),
            },
        );
        let _ = device.send_event(wl_data_device::Event::DataOffer { id: offer.clone() });
        for mime_type in selection.mime_types {
            let _ = offer.send_event(wl_data_offer::Event::Offer { mime_type });
        }
        let _ = device.send_event(wl_data_device::Event::Selection { id: Some(offer) });
    }

    fn receive_clipboard_offer(
        &mut self,
        offer: &wl_data_offer::WlDataOffer,
        client_id: &ClientId,
        source_generation: u64,
        mime_type: String,
        fd: OwnedFd,
    ) {
        let Some(binding) = self.data_offers.get(&offer.id()) else {
            return;
        };
        let Some(selection) = self.active_clipboard.as_ref() else {
            return;
        };
        if binding.target_client_id != *client_id
            || binding.source_generation != selection.generation
            || source_generation != selection.generation
            || !binding.mime_types.iter().any(|mime| mime == &mime_type)
        {
            return;
        }
        match &selection.source {
            ClipboardSourceBackend::InternalWayland { source, client_id } => {
                let active_source_client_matches = self
                    .data_sources
                    .get(&source.id())
                    .is_some_and(|registered| registered.client_id == *client_id);
                if !active_source_client_matches || !source.is_alive() {
                    return;
                }
                let _ = source.send_event(wl_data_source::Event::Send {
                    mime_type,
                    fd: fd.as_fd(),
                });
            }
            ClipboardSourceBackend::HostBridge { offer_id } => {
                if let Some(bridge) = self.clipboard_bridge.as_mut() {
                    let _ = bridge.request_host_data(*offer_id, mime_type, fd);
                }
            }
        }
    }

    fn register_surface_resource(&mut self, surface_id: u32, surface: wl_surface::WlSurface) {
        self.surface_resources.entry(surface_id).or_insert(surface);
    }

    fn register_output_resource(&mut self, output: wl_output::WlOutput) {
        if self
            .output_resources
            .iter()
            .any(|resource| same_wayland_resource(resource, &output))
        {
            return;
        }

        send_output_description(
            &output,
            self.output_size,
            self.output_scale,
            self.output_refresh,
        );
        self.output_resources.push(output);
    }

    fn unregister_output_resource(&mut self, output: &wl_output::WlOutput) {
        let output_id = output.id().protocol_id();
        self.output_resources
            .retain(|resource| !same_wayland_resource(resource, output));
        self.surface_entered_outputs
            .retain(|(_, entered_output_id)| *entered_output_id != output_id);
    }

    fn send_output_mode_to_bound_outputs(&self) {
        for output in &self.output_resources {
            send_output_mode(output, self.output_size, self.output_refresh);
            send_output_done_if_supported(output);
        }
    }

    fn send_output_scale_to_bound_outputs(&self) {
        for output in &self.output_resources {
            send_output_scale(output, self.output_scale);
            send_output_done_if_supported(output);
        }
    }

    fn register_fractional_scale_resource(
        &mut self,
        surface: &wl_surface::WlSurface,
        fractional_scale: wp_fractional_scale_v1::WpFractionalScaleV1,
    ) {
        let surface_id = compositor_surface_id(surface);

        fractional_scale.preferred_scale(self.output_scale.preferred_scale());
        self.fractional_scale_resources
            .entry(surface_id)
            .or_default()
            .push(fractional_scale);
    }

    fn unregister_fractional_scale_resources_for_surface(&mut self, surface_id: u32) {
        self.fractional_scale_resources.remove(&surface_id);
    }

    fn unregister_fractional_scale_resource(&mut self, surface_id: u32, resource_id: u32) {
        if let Some(resources) = self.fractional_scale_resources.get_mut(&surface_id) {
            resources.retain(|resource| resource.id().protocol_id() != resource_id);
            if resources.is_empty() {
                self.fractional_scale_resources.remove(&surface_id);
            }
        }
    }

    fn send_fractional_scale_to_bound_surfaces(&self) {
        for fractional_scales in self.fractional_scale_resources.values() {
            for fractional_scale in fractional_scales {
                fractional_scale.preferred_scale(self.output_scale.preferred_scale());
            }
        }
    }

    fn ensure_surface_entered_outputs(&mut self, surface: &wl_surface::WlSurface) {
        let surface_id = compositor_surface_id(surface);
        for output in &self.output_resources {
            if !resource_belongs_to_surface_client(output, surface) {
                continue;
            }
            let output_id = output.id().protocol_id();
            if !self.surface_entered_outputs.insert((surface_id, output_id)) {
                continue;
            }
            let _ = surface.send_event(wl_surface::Event::Enter {
                output: output.clone(),
            });
        }
    }

    fn reconfigure_stateful_windows_for_output_size(&mut self) {
        let toplevels = self
            .toplevel_surfaces
            .iter()
            .filter_map(|(surface_id, toplevel)| {
                let mode = toplevel.window.mode();
                (mode != ToplevelMode::Floating && !toplevel.window.is_minimized())
                    .then_some((*surface_id, mode))
            })
            .collect::<Vec<_>>();

        for (surface_id, mode) in toplevels {
            self.send_configure_root_window_to(
                surface_id,
                self.output_size.width,
                self.output_size.height,
                mode.xdg_states(),
            );
        }
    }

    fn unregister_surface_resource(&mut self, surface_id: u32) {
        self.cancel_pending_acquire_commits_for_surface(
            surface_id,
            AcquireWatchCancelReason::SurfaceDestroyed,
        );
        self.discard_pending_presentation_feedbacks_for_surface(surface_id);
        self.deactivate_pointer_constraints_for_surface(surface_id, false);
        self.cleanup_subsurface_stack_state_for_surface(surface_id);
        self.surface_resources.remove(&surface_id);
        self.cursor_surface_ids.remove(&surface_id);
        let removed_cursor_content = self.client_cursor_surfaces.remove(&surface_id).is_some();
        let active_cursor_pointer = self
            .active_client_cursor
            .as_ref()
            .filter(|active| active.surface_id == surface_id)
            .map(|active| active.pointer.clone());
        if let Some(pointer) = active_cursor_pointer {
            self.active_client_cursor = None;
            self.cursor_visibility.client_cursor_pointer = None;
            self.cursor_visibility.client_hidden_pointer = Some(pointer);
            pointer_debug_log(format!(
                "cursor cleanup surface={} reason=active-surface-destroyed",
                surface_id
            ));
            self.advance_render_generation(RenderGenerationCause::CursorState);
            self.sync_cursor_visibility_request();
        } else if removed_cursor_content {
            pointer_debug_log(format!(
                "cursor cleanup surface={} reason=inactive-surface-destroyed",
                surface_id
            ));
        }
        self.unregister_fractional_scale_resources_for_surface(surface_id);
        self.surface_placements.remove(&surface_id);
        self.current_surface_buffers.remove(&surface_id);
        self.surface_window_geometries.remove(&surface_id);
        self.configured_xdg_surfaces.remove(&surface_id);
        self.surface_entered_outputs
            .retain(|(entered_surface_id, _)| *entered_surface_id != surface_id);
        self.toplevel_surfaces.remove(&surface_id);
        self.popup_surfaces.remove(&surface_id);
        self.clear_resize_state_for_surfaces(&[surface_id]);
        self.surface_placements
            .retain(|_, placement| placement.parent_surface_id != Some(surface_id));
        let mut removed_surface_ids = vec![surface_id];
        removed_surface_ids.extend(
            self.renderable_surfaces
                .iter()
                .filter(|surface| surface.placement.parent_surface_id == Some(surface_id))
                .map(|surface| surface.surface_id),
        );
        removed_surface_ids.sort_unstable();
        removed_surface_ids.dedup();
        self.popup_grab_stack
            .retain(|surface_id| !removed_surface_ids.contains(surface_id));
        self.recent_input_serials
            .retain(|input| !removed_surface_ids.contains(&compositor_surface_id(&input.surface)));
        self.clear_pointer_button_state_for_removed_surfaces(
            &removed_surface_ids,
            "surface-destroyed",
        );

        for removed_surface_id in &removed_surface_ids {
            self.popup_surfaces.remove(removed_surface_id);
            if let Some(buffer) = self.active_dmabuf_buffers.remove(removed_surface_id) {
                self.queue_dmabuf_buffer_release(buffer);
            }
        }
        let previous_renderable_count = self.renderable_surfaces.len();
        self.renderable_surfaces.retain(|surface| {
            surface.surface_id != surface_id
                && surface.placement.parent_surface_id != Some(surface_id)
        });
        if self.renderable_surfaces.len() != previous_renderable_count {
            self.advance_render_generation(RenderGenerationCause::SurfaceUnmap);
        }

        if self
            .focused_surface
            .as_ref()
            .is_some_and(|surface| compositor_surface_id(surface) == surface_id)
        {
            self.focused_surface = None;
        }

        if self
            .keyboard_surface
            .as_ref()
            .is_some_and(|surface| compositor_surface_id(surface) == surface_id)
        {
            self.keyboard_surface = None;
        }

        if self
            .pointer_surface
            .as_ref()
            .is_some_and(|surface| removed_surface_ids.contains(&compositor_surface_id(surface)))
        {
            self.pointer_surface = None;
            self.clear_pointer_constraint();
            self.cursor_visibility.client_hidden_pointer = None;
            self.cursor_visibility.client_cursor_pointer = None;
            self.sync_cursor_visibility_request();
        }
        self.pointer_entered_surfaces
            .retain(|(_, surface)| !removed_surface_ids.contains(&compositor_surface_id(surface)));
        self.pointer_enter_serials
            .retain(|entry| !removed_surface_ids.contains(&compositor_surface_id(&entry.surface)));
    }

    fn clear_pointer_button_state_for_removed_surfaces(
        &mut self,
        removed_surface_ids: &[u32],
        reason: &'static str,
    ) {
        self.cancel_implicit_pointer_grab_for_surface_ids(removed_surface_ids, reason);
        self.held_pointer_buttons.retain(|press| {
            !removed_surface_ids.contains(&compositor_surface_id(&press.surface))
                && !removed_surface_ids.contains(&press.root_surface_id)
        });
        if self.last_pointer_press.as_ref().is_some_and(|press| {
            removed_surface_ids.contains(&compositor_surface_id(&press.surface))
                || removed_surface_ids.contains(&press.root_surface_id)
        }) {
            self.last_pointer_press = None;
        }
    }

    fn register_keyboard(&mut self, keyboard: wl_keyboard::WlKeyboard) {
        if self
            .keyboard_resources
            .iter()
            .any(|resource| same_wayland_resource(resource, &keyboard))
        {
            return;
        }
        self.keyboard_resources.push(keyboard);
        if let Some(surface) = self.focused_surface.clone() {
            self.ensure_keyboard_focus(&surface);
        }
    }

    fn register_pointer(&mut self, pointer: wl_pointer::WlPointer) {
        if self
            .pointer_resources
            .iter()
            .any(|resource| same_wayland_resource(resource, &pointer))
        {
            return;
        }
        self.pointer_resources.push(pointer.clone());
        self.synchronize_pointer_resource_focus(&pointer);
    }

    fn unregister_keyboard(&mut self, keyboard: &wl_keyboard::WlKeyboard) {
        self.keyboard_resources
            .retain(|resource| !same_wayland_resource(resource, keyboard));
    }

    fn unregister_pointer(&mut self, pointer: &wl_pointer::WlPointer) {
        self.pointer_resources
            .retain(|resource| !same_wayland_resource(resource, pointer));
        self.pointer_entered_surfaces
            .retain(|(resource, _)| !same_wayland_resource(resource, pointer));
        self.pointer_enter_serials
            .retain(|entry| !same_wayland_resource(&entry.pointer, pointer));
        self.relative_pointer_resources
            .retain(|resource| !same_wayland_resource(&resource.source_pointer, pointer));
        self.deactivate_pointer_constraints_for_pointer(pointer, false);
        if self
            .active_client_cursor
            .as_ref()
            .is_some_and(|active| same_wayland_resource(&active.pointer, pointer))
        {
            self.active_client_cursor = None;
            self.advance_render_generation(RenderGenerationCause::CursorState);
            pointer_debug_log(format!(
                "cursor cleanup pointer={} reason=owning-pointer-destroyed",
                pointer.id().protocol_id()
            ));
        }
        if self
            .cursor_visibility
            .client_hidden_pointer
            .as_ref()
            .is_some_and(|hidden_pointer| same_wayland_resource(hidden_pointer, pointer))
        {
            self.cursor_visibility.client_hidden_pointer = None;
            self.sync_cursor_visibility_request();
        }
        if self
            .cursor_visibility
            .client_cursor_pointer
            .as_ref()
            .is_some_and(|cursor_pointer| same_wayland_resource(cursor_pointer, pointer))
        {
            self.cursor_visibility.client_cursor_pointer = None;
            self.sync_cursor_visibility_request();
        }
    }

    fn set_pointer_cursor(
        &mut self,
        pointer: &wl_pointer::WlPointer,
        serial: u32,
        surface: Option<wl_surface::WlSurface>,
        hotspot_x: i32,
        hotspot_y: i32,
    ) {
        let Some(pointer_surface) = self.pointer_surface.as_ref() else {
            return;
        };
        let focused_client = resource_belongs_to_surface_client(pointer, pointer_surface);
        let exact_serial = self.pointer_has_current_enter_serial(pointer, serial, pointer_surface);
        let valid = focused_client && exact_serial;
        pointer_debug_log(format!(
            "cursor request pointer={} client={} serial={} valid={} exact_serial={} focused_client={} null={}",
            pointer.id().protocol_id(),
            wayland_resource_client_label(pointer),
            serial,
            valid,
            exact_serial,
            focused_client,
            surface.is_none()
        ));
        if !valid {
            pointer_debug_log("cursor request ignored reason=invalid-focus-or-enter-serial");
            return;
        }
        let resolves_pending_unlock = self
            .pending_locked_pointer_reveal
            .as_ref()
            .is_some_and(|pending| same_wayland_resource(&pending.pointer, pointer));
        let Some(surface) = surface else {
            let changed = self.active_client_cursor.take().is_some();
            self.cursor_visibility.client_hidden_pointer = Some(pointer.clone());
            self.cursor_visibility.client_cursor_pointer = None;
            if changed {
                self.advance_render_generation(RenderGenerationCause::CursorState);
            }
            self.sync_cursor_visibility_request();
            if resolves_pending_unlock {
                self.finalize_pending_locked_pointer_reveal("client_hidden_cursor");
            }
            return;
        };
        let surface_id = compositor_surface_id(&surface);
        self.cursor_surface_ids.insert(surface_id);
        self.unmap_surface_content(surface_id);
        let changed = self.active_client_cursor.as_ref().is_none_or(|active| {
            !same_wayland_resource(&active.pointer, pointer)
                || active.surface_id != surface_id
                || active.hotspot_x != hotspot_x
                || active.hotspot_y != hotspot_y
        });
        self.active_client_cursor = Some(ActiveClientCursor {
            pointer: pointer.clone(),
            surface_id,
            hotspot_x,
            hotspot_y,
        });
        self.cursor_visibility.client_hidden_pointer = None;
        self.cursor_visibility.client_cursor_pointer = Some(pointer.clone());
        pointer_debug_log(format!(
            "cursor request client_surface pointer={} surface={} hotspot=({}, {})",
            pointer.id().protocol_id(),
            surface_id,
            hotspot_x,
            hotspot_y
        ));
        if changed {
            self.advance_render_generation(RenderGenerationCause::CursorState);
        }
        self.sync_cursor_visibility_request();
        if resolves_pending_unlock {
            self.finalize_pending_locked_pointer_reveal("client_cursor_surface");
        }
    }

    fn is_cursor_surface(&self, surface_id: u32) -> bool {
        self.cursor_surface_ids.contains(&surface_id)
    }

    fn client_cursor_render_state(&self) -> Option<ClientCursorRenderState<'_>> {
        if self.cursor_visibility.lock_hidden_constraint_id.is_some() {
            return None;
        }
        let active = self.active_client_cursor.as_ref()?;
        let surface = self.client_cursor_surfaces.get(&active.surface_id)?;
        Some(ClientCursorRenderState {
            surface,
            logical_x: (self.last_pointer_x.round() as i32).saturating_sub(active.hotspot_x),
            logical_y: (self.last_pointer_y.round() as i32).saturating_sub(active.hotspot_y),
        })
    }

    fn active_client_cursor_has_content(&self) -> bool {
        self.active_client_cursor
            .as_ref()
            .is_some_and(|active| self.client_cursor_surfaces.contains_key(&active.surface_id))
    }

    fn send_keyboard_key(&mut self, key: u32, pressed: bool) {
        let modifiers_changed = self.keyboard_modifiers.update_key(key, pressed);
        let Some(surface) = self.focused_surface.clone() else {
            return;
        };
        let state = if pressed {
            wl_keyboard::KeyState::Pressed
        } else {
            wl_keyboard::KeyState::Released
        };
        let time = wayland_event_time();

        self.ensure_keyboard_focus(&surface);

        let serial = self.next_configure_serial();
        self.remember_input_serial(serial, surface.clone());
        for keyboard in self
            .keyboard_resources
            .iter()
            .filter(|keyboard| resource_belongs_to_surface_client(*keyboard, &surface))
        {
            let _ = keyboard.send_event(wl_keyboard::Event::Key {
                serial,
                time,
                key,
                state: WEnum::Value(state),
            });
        }
        if modifiers_changed {
            self.send_keyboard_modifiers(&surface, serial);
        }
    }

    fn ensure_keyboard_focus(&mut self, surface: &wl_surface::WlSurface) {
        if self
            .keyboard_surface
            .as_ref()
            .is_some_and(|current| same_surface_resource(current, surface))
        {
            return;
        }

        self.clear_keyboard_focus();
        self.keyboard_resources.retain(Resource::is_alive);
        let keyboards = self
            .keyboard_resources
            .iter()
            .filter(|keyboard| resource_belongs_to_surface_client(*keyboard, surface))
            .cloned()
            .collect::<Vec<_>>();
        if keyboards.is_empty() {
            return;
        }

        let serial = self.next_configure_serial();
        for keyboard in keyboards {
            let _ = keyboard.send_event(wl_keyboard::Event::Enter {
                serial,
                surface: surface.clone(),
                keys: Vec::new(),
            });
            let _ = keyboard.send_event(wl_keyboard::Event::Modifiers {
                serial,
                mods_depressed: self.keyboard_modifiers.mods_depressed(),
                mods_latched: 0,
                mods_locked: self.keyboard_modifiers.mods_locked(),
                group: 0,
            });
        }
        pointer_debug_log(format!(
            "keyboard enter surface={} client={}",
            compositor_surface_id(surface),
            wayland_resource_client_label(surface)
        ));
        self.keyboard_surface = Some(surface.clone());
        self.publish_clipboard_to_focused_client();
    }

    fn send_keyboard_modifiers(&mut self, surface: &wl_surface::WlSurface, serial: u32) {
        self.keyboard_resources.retain(Resource::is_alive);
        for keyboard in self
            .keyboard_resources
            .iter()
            .filter(|keyboard| resource_belongs_to_surface_client(*keyboard, surface))
        {
            let _ = keyboard.send_event(wl_keyboard::Event::Modifiers {
                serial,
                mods_depressed: self.keyboard_modifiers.mods_depressed(),
                mods_latched: 0,
                mods_locked: self.keyboard_modifiers.mods_locked(),
                group: 0,
            });
        }
    }

    fn clear_keyboard_focus(&mut self) {
        let Some(surface) = self.keyboard_surface.take() else {
            return;
        };
        self.keyboard_resources.retain(Resource::is_alive);
        let keyboards = self
            .keyboard_resources
            .iter()
            .filter(|keyboard| resource_belongs_to_surface_client(*keyboard, &surface))
            .cloned()
            .collect::<Vec<_>>();
        if keyboards.is_empty() {
            return;
        }

        let serial = self.next_configure_serial();
        for keyboard in keyboards {
            let _ = keyboard.send_event(wl_keyboard::Event::Leave {
                serial,
                surface: surface.clone(),
            });
        }
        pointer_debug_log(format!(
            "keyboard leave surface={} client={}",
            compositor_surface_id(&surface),
            wayland_resource_client_label(&surface)
        ));
    }

    fn send_pointer_motion(&mut self, x: f64, y: f64) {
        if let Some(active) = self.active_locked_pointer_binding() {
            pointer_debug_log(format!(
                "pointer.motion locked=true absolute_suppressed=true requested_output=({},{}) anchor_output=({},{})",
                x, y, active.activation_anchor.x, active.activation_anchor.y
            ));
            self.pin_locked_pointer_focus(&active);
            return;
        }
        if self.active_confined_pointer_binding().is_some() {
            self.send_confined_pointer_motion(x, y);
            return;
        }
        self.update_pointer_position(x, y);
        if self.send_implicit_pointer_grab_motion(x, y) {
            return;
        }
        let Some(target) = self.pointer_target_at(x, y) else {
            self.clear_pointer_focus();
            return;
        };
        if !self.pointer_target_allowed_by_popup_grab(&target) {
            self.clear_pointer_focus();
            return;
        }
        let time = wayland_event_time();
        self.ensure_pointer_focus(&target.surface);
        self.send_pointer_enter_if_needed(&target);

        for pointer in self
            .pointer_resources
            .iter()
            .filter(|pointer| resource_belongs_to_surface_client(*pointer, &target.surface))
        {
            let _ = pointer.send_event(wl_pointer::Event::Motion {
                time,
                surface_x: target.surface_x,
                surface_y: target.surface_y,
            });
            send_pointer_frame_if_supported(pointer);
        }
    }

    fn update_pointer_position(&mut self, x: f64, y: f64) {
        let changed = self.last_pointer_x != x || self.last_pointer_y != y;
        let moves_client_cursor = changed && self.client_cursor_render_state().is_some();
        self.last_pointer_x = x;
        self.last_pointer_y = y;
        if moves_client_cursor {
            self.advance_render_generation(RenderGenerationCause::CursorMotion);
        }
    }

    fn update_pointer_position_without_client_dispatch(&mut self, x: f64, y: f64) -> bool {
        let before = self.render_generation;
        self.update_pointer_position(x, y);
        self.render_generation != before
    }

    fn send_pointer_motion_sample(&mut self, sample: PointerMotionSample) {
        self.last_pointer_motion_usec = Some(sample.timestamp_usec);
        if let Some(relative) = sample.relative {
            self.last_relative_pointer_motion = Some(relative);
            self.send_relative_pointer_motion(sample.timestamp_usec, relative);
        }
        if let Some(position) = sample.absolute {
            let locked_surface_id = self
                .pointer_surface
                .as_ref()
                .map(compositor_surface_id)
                .filter(|surface_id| self.pointer_constraint.filters_absolute_motion(*surface_id));
            if locked_surface_id.is_none() {
                self.send_pointer_motion(position.x, position.y);
            } else if let Some(surface_id) = locked_surface_id {
                pointer_debug_log(format!(
                    "pointer.motion locked=true absolute_suppressed=true output=({},{}) surface={}",
                    position.x, position.y, surface_id
                ));
            }
        }
    }

    #[cfg(test)]
    fn activate_pointer_constraint_for_focused_surface(
        &mut self,
        mode: PointerConstraintMode,
    ) -> bool {
        let Some(surface) = self.pointer_surface.as_ref() else {
            return false;
        };
        self.pointer_constraint
            .activate(mode, compositor_surface_id(surface));
        true
    }

    fn clear_pointer_constraint(&mut self) {
        self.pointer_constraint.clear();
    }

    fn sync_cursor_visibility_request(&mut self) {
        let desired_visible = self.cursor_visibility.desired_visible();
        if self.cursor_visibility.visible == desired_visible {
            return;
        }
        self.cursor_visibility.visible = desired_visible;
        pointer_debug_log(format!(
            "cursor visibility effective visible={} client_hidden={} lock_hidden={:?}",
            desired_visible,
            self.cursor_visibility
                .client_hidden_pointer
                .as_ref()
                .map(|pointer| pointer.id().protocol_id())
                .map_or_else(|| "none".to_string(), |id| id.to_string()),
            self.cursor_visibility.lock_hidden_constraint_id
        ));
        self.pending_pointer_constraint_backend_requests.push(
            PointerConstraintBackendRequest::ApplyCursorVisibility {
                visible: desired_visible,
            },
        );
    }

    fn begin_client_dispatch_cycle(&mut self) {
        self.dispatch_epoch = self.dispatch_epoch.saturating_add(1);
    }

    fn finish_client_dispatch_cycle(&mut self) {
        self.finalize_pending_locked_pointer_reveal_after_dispatch();
    }

    fn begin_pending_locked_pointer_reveal(
        &mut self,
        backend_id: PointerConstraintBackendId,
        pointer: wl_pointer::WlPointer,
        surface: wl_surface::WlSurface,
        fallback_position: Option<OutputPosition>,
    ) {
        pointer_debug_log(format!(
            "pointer.unlock transition_begin id={} generation={} fallback=({}) epoch={} cursor_kept_hidden=true",
            backend_id.constraint_id,
            backend_id.generation,
            fallback_position
                .map(|position| format!("{},{}", position.x, position.y))
                .unwrap_or_else(|| "none".to_string()),
            self.dispatch_epoch
        ));
        self.pending_locked_pointer_reveal = Some(PendingLockedPointerReveal {
            backend_id,
            pointer,
            surface,
            fallback_position,
            created_dispatch_epoch: self.dispatch_epoch,
        });
    }

    fn cancel_pending_locked_pointer_reveal_for_id(
        &mut self,
        id: PointerConstraintBackendId,
        reason: &str,
    ) {
        if self
            .pending_locked_pointer_reveal
            .as_ref()
            .is_some_and(|pending| pending.backend_id == id)
        {
            pointer_debug_log(format!(
                "pointer.unlock transition_cancel id={} generation={} reason={}",
                id.constraint_id, id.generation, reason
            ));
            self.pending_locked_pointer_reveal = None;
        }
    }

    fn cancel_pending_locked_pointer_reveal_for_constraint(
        &mut self,
        constraint_id: u64,
        reason: &str,
    ) {
        if self
            .pending_locked_pointer_reveal
            .as_ref()
            .is_some_and(|pending| pending.backend_id.constraint_id == constraint_id)
        {
            pointer_debug_log(format!(
                "pointer.unlock transition_cancel id={} reason={}",
                constraint_id, reason
            ));
            self.pending_locked_pointer_reveal = None;
        }
    }

    fn pending_locked_pointer_reveal_matches(
        &self,
        pointer: &wl_pointer::WlPointer,
        surface: &wl_surface::WlSurface,
    ) -> bool {
        self.pending_locked_pointer_reveal
            .as_ref()
            .is_some_and(|pending| {
                same_wayland_resource(&pending.pointer, pointer)
                    && same_surface_resource(&pending.surface, surface)
            })
    }

    fn finalize_pending_locked_pointer_reveal(&mut self, reason: &str) {
        let Some(pending) = self.pending_locked_pointer_reveal.take() else {
            return;
        };
        if self.cursor_visibility.lock_hidden_constraint_id
            == Some(pending.backend_id.constraint_id)
        {
            self.cursor_visibility.lock_hidden_constraint_id = None;
            if self.active_client_cursor_has_content() {
                self.advance_render_generation(RenderGenerationCause::CursorState);
            }
        }
        pointer_debug_log(format!(
            "pointer.unlock transition_finalize reason={} id={} generation={} final=({}) visibility_request={} epoch={}",
            reason,
            pending.backend_id.constraint_id,
            pending.backend_id.generation,
            pending
                .fallback_position
                .map(|position| format!("{},{}", position.x, position.y))
                .unwrap_or_else(|| format!("{},{}", self.last_pointer_x, self.last_pointer_y)),
            self.cursor_visibility.desired_visible(),
            self.dispatch_epoch
        ));
        self.sync_cursor_visibility_request();
    }

    fn finalize_pending_locked_pointer_reveal_after_dispatch(&mut self) {
        let should_finalize = self
            .pending_locked_pointer_reveal
            .as_ref()
            .is_some_and(|pending| {
                pending.created_dispatch_epoch.saturating_add(2) < self.dispatch_epoch
            });
        if should_finalize {
            self.finalize_pending_locked_pointer_reveal("dispatch_cycle_fallback");
        }
    }

    fn allocate_internal_pointer_constraint_id(&mut self) -> u64 {
        self.next_internal_pointer_constraint_id = self
            .next_internal_pointer_constraint_id
            .saturating_add(1)
            .max(1);
        self.next_internal_pointer_constraint_id
    }

    fn active_locked_pointer_binding(&self) -> Option<ActiveLockedPointerRouting> {
        let active = self.active_locked_pointer_routing.as_ref()?;
        let constraint = self.pointer_constraints.get(&active.constraint_id)?;
        if constraint.generation != active.generation
            || !constraint.active
            || constraint.defunct
            || constraint.mode != PointerConstraintMode::Locked
        {
            return None;
        }
        if !active.pointer.is_alive() || !active.surface.is_alive() {
            return None;
        }
        Some(active.clone())
    }

    fn clear_active_locked_pointer_routing(&mut self) {
        self.active_locked_pointer_routing = None;
    }

    fn pin_locked_pointer_focus(&mut self, active: &ActiveLockedPointerRouting) {
        self.ensure_pointer_focus(&active.surface);
        if !self.pointer_resource_entered_surface(&active.pointer, &active.surface) {
            let target = PointerTarget {
                surface: active.surface.clone(),
                surface_x: active.surface_x,
                surface_y: active.surface_y,
            };
            self.send_pointer_enter_to_resource(&active.pointer, &target);
        }
    }

    fn locked_pointer_input_surface(&self) -> Option<wl_surface::WlSurface> {
        self.active_locked_pointer_binding()
            .map(|active| active.surface)
    }

    fn active_confined_pointer_binding(&self) -> Option<ActiveConfinedPointerRouting> {
        let active = self.active_confined_pointer_routing.as_ref()?;
        let constraint = self.pointer_constraints.get(&active.constraint_id)?;
        if constraint.generation != active.generation
            || !constraint.active
            || constraint.defunct
            || constraint.mode != PointerConstraintMode::Confined
        {
            return None;
        }
        if !active.pointer.is_alive() || !active.surface.is_alive() {
            return None;
        }
        Some(active.clone())
    }

    fn clear_active_confined_pointer_routing(&mut self) {
        self.active_confined_pointer_routing = None;
    }

    fn pin_confined_pointer_focus(&mut self, active: &ActiveConfinedPointerRouting) {
        if !self
            .pointer_surface
            .as_ref()
            .is_some_and(|current| same_surface_resource(current, &active.surface))
        {
            self.pointer_surface = Some(active.surface.clone());
        }
        if !self.pointer_resource_entered_surface(&active.pointer, &active.surface) {
            let target = self
                .pointer_target_for_surface_at_output(
                    &active.surface,
                    self.last_pointer_x,
                    self.last_pointer_y,
                )
                .unwrap_or(PointerTarget {
                    surface: active.surface.clone(),
                    surface_x: 0.0,
                    surface_y: 0.0,
                });
            self.send_pointer_enter_to_resource(&active.pointer, &target);
        }
    }

    fn send_confined_pointer_motion(&mut self, x: f64, y: f64) {
        let Some(active) = self.active_confined_pointer_binding() else {
            return;
        };
        let proposed = OutputPosition { x, y };
        let clamped = active.region.closest_point(proposed);
        self.update_pointer_position(clamped.x, clamped.y);
        self.pin_confined_pointer_focus(&active);
        let Some(target) =
            self.pointer_target_for_surface_at_output(&active.surface, clamped.x, clamped.y)
        else {
            pointer_debug_log(format!(
                "confined motion dropped id={} reason=local_unresolved proposed=({},{}) clamped=({},{})",
                active.constraint_id, x, y, clamped.x, clamped.y
            ));
            return;
        };
        pointer_debug_log(format!(
            "confined motion proposed=({},{}) clamped=({},{}) surface_local=({},{})",
            x, y, clamped.x, clamped.y, target.surface_x, target.surface_y
        ));
        let time = wayland_event_time();
        for pointer in self
            .pointer_resources
            .iter()
            .filter(|pointer| resource_belongs_to_surface_client(*pointer, &active.surface))
        {
            let _ = pointer.send_event(wl_pointer::Event::Motion {
                time,
                surface_x: target.surface_x,
                surface_y: target.surface_y,
            });
            send_pointer_frame_if_supported(pointer);
        }
    }

    fn register_pointer_constraint(&mut self, registration: PointerConstraintRegistration) -> bool {
        if let Some(existing) = self.pointer_constraints.values().find(|constraint| {
            !constraint.defunct && same_surface_resource(&constraint.surface, &registration.surface)
        }) {
            pointer_debug_log(format!(
                "constraint reject already_constrained existing={} requested={} surface={} pointer={}",
                existing.id,
                registration.id,
                compositor_surface_id(&registration.surface),
                registration.pointer.id().protocol_id()
            ));
            return false;
        }

        self.next_pointer_constraint_generation = self
            .next_pointer_constraint_generation
            .wrapping_add(1)
            .max(1);
        let generation = self.next_pointer_constraint_generation;
        pointer_debug_log(format!(
            "constraint create id={} generation={} mode={:?} surface={} pointer={} client={}",
            registration.id,
            generation,
            registration.mode,
            compositor_surface_id(&registration.surface),
            registration.pointer.id().protocol_id(),
            wayland_resource_client_label(&registration.pointer)
        ));
        self.pointer_constraints.insert(
            registration.id,
            PointerConstraint {
                id: registration.id,
                generation,
                mode: registration.mode,
                lifetime: registration.lifetime,
                surface: registration.surface,
                pointer: registration.pointer,
                locked_resource: registration.locked_resource,
                confined_resource: registration.confined_resource,
                active: false,
                backend_pending: false,
                defunct: false,
                pending_region: registration.region.clone(),
                committed_region: registration.region,
                pending_cursor_position_hint: None,
                committed_cursor_position_hint: None,
            },
        );
        self.maybe_request_pointer_constraint_activation(registration.id);
        true
    }

    fn maybe_request_pointer_constraint_activation(&mut self, constraint_id: u64) {
        let Some((pointer, surface)) =
            self.pointer_constraints
                .get(&constraint_id)
                .and_then(|constraint| {
                    if constraint.active || constraint.backend_pending || constraint.defunct {
                        return None;
                    }
                    Some((constraint.pointer.clone(), constraint.surface.clone()))
                })
        else {
            return;
        };
        if !pointer.is_alive() || !surface.is_alive() {
            return;
        }
        let Some(focused) = self.pointer_surface.clone() else {
            return;
        };
        if !resource_belongs_to_surface_client(&pointer, &focused)
            || !resource_belongs_to_surface_client(&pointer, &surface)
            || self.root_surface_id_for_surface(compositor_surface_id(&focused))
                != self.root_surface_id_for_surface(compositor_surface_id(&surface))
        {
            pointer_debug_log(format!(
                "pointer.constraint activation deferred id={} reason=focus_client_or_root_mismatch focused={} owner={}",
                constraint_id,
                compositor_surface_id(&focused),
                compositor_surface_id(&surface)
            ));
            return;
        }
        if self.active_backend_constraint.is_some() || self.pending_backend_constraint.is_some() {
            pointer_debug_log(format!(
                "backend activate requested id={} skipped current_active={:?} current_pending={:?}",
                constraint_id, self.active_backend_constraint, self.pending_backend_constraint
            ));
            return;
        }
        let confinement_region = self.pointer_constraint_output_region(constraint_id);
        let request = {
            let Some(constraint) = self.pointer_constraints.get(&constraint_id) else {
                return;
            };
            let backend_id = constraint.backend_id();
            match constraint.mode {
                PointerConstraintMode::Locked => {
                    let Some(anchor) = self.pointer_constraint_activation_anchor(
                        constraint_id,
                        confinement_region.as_ref(),
                    ) else {
                        pointer_debug_log(format!(
                            "constraint activation skipped id={} reason=anchor_unresolved",
                            constraint.id
                        ));
                        return;
                    };
                    let target = self
                        .pointer_target_for_grabbed_surface_at_output(&surface, anchor.x, anchor.y)
                        .unwrap_or(PointerTarget {
                            surface: surface.clone(),
                            surface_x: anchor.x,
                            surface_y: anchor.y,
                        });
                    self.ensure_pointer_focus(&surface);
                    self.send_pointer_enter_to_resource(&pointer, &target);
                    PointerConstraintBackendRequest::ActivateLocked {
                        id: backend_id,
                        anchor,
                    }
                }
                PointerConstraintMode::Confined => {
                    let Some(region) = confinement_region else {
                        pointer_debug_log(format!(
                            "constraint activation skipped id={} reason=region_unresolved mode={:?}",
                            constraint.id, constraint.mode
                        ));
                        return;
                    };
                    PointerConstraintBackendRequest::ActivateConfined {
                        id: backend_id,
                        region,
                    }
                }
                PointerConstraintMode::None => PointerConstraintBackendRequest::Deactivate {
                    id: backend_id,
                    restore_position: None,
                },
            }
        };
        let Some(constraint) = self.pointer_constraints.get_mut(&constraint_id) else {
            return;
        };
        if constraint.active || constraint.backend_pending || constraint.defunct {
            return;
        }
        let backend_id = constraint.backend_id();
        self.pending_backend_constraint = Some(backend_id);
        constraint.backend_pending = true;
        if let PointerConstraintBackendRequest::ActivateLocked { anchor, .. } = &request {
            self.pending_locked_activation_anchors
                .insert(backend_id, *anchor);
        } else {
            self.pending_locked_activation_anchors.remove(&backend_id);
        }
        pointer_debug_log(format!(
            "constraint activation queued id={} generation={}",
            backend_id.constraint_id, backend_id.generation
        ));
        self.pending_pointer_constraint_backend_requests
            .push(request);
    }

    fn pointer_constraint_activation_anchor(
        &self,
        constraint_id: u64,
        region: Option<&OutputRegion>,
    ) -> Option<OutputPosition> {
        let constraint = self.pointer_constraints.get(&constraint_id)?;
        let current = OutputPosition {
            x: self.last_pointer_x,
            y: self.last_pointer_y,
        };
        let Some(region) = region else {
            return Some(current);
        };
        if region.closest_point(current) == current {
            return Some(current);
        }
        let owner_root =
            self.root_surface_id_for_surface(compositor_surface_id(&constraint.surface));
        if let Some(press) = self.held_pointer_buttons.iter().rev().find(|press| {
            press.root_surface_id == owner_root
                && resource_belongs_to_surface_client(&press.surface, &constraint.surface)
        }) {
            let pressed = OutputPosition {
                x: press.output_x,
                y: press.output_y,
            };
            if region.closest_point(pressed) == pressed {
                return Some(pressed);
            }
        }
        Some(region.closest_point(current))
    }

    fn pointer_constraint_output_region(&mut self, constraint_id: u64) -> Option<OutputRegion> {
        let (surface_id, constraint_region, surface_resource) = self
            .pointer_constraints
            .get(&constraint_id)
            .map(|constraint| {
                (
                    compositor_surface_id(&constraint.surface),
                    constraint.committed_region.clone(),
                    constraint.surface.clone(),
                )
            })?;
        self.refresh_surface_origin_cache();
        let index = self
            .renderable_surfaces
            .iter()
            .position(|renderable| renderable.surface_id == surface_id)?;
        let renderable = &self.renderable_surfaces[index];
        let origin = self.surface_origin_cache.get(index).copied()?;
        let input_region = surface_resource
            .data::<SurfaceData>()
            .map(|data| {
                let mut rows = Vec::new();
                for y in 0..renderable.height {
                    let mut run_start = None;
                    for x in 0..renderable.width {
                        let surface_x = f64::from(x);
                        let surface_y = f64::from(y);
                        let contained = constraint_region.contains(
                            surface_x,
                            surface_y,
                            renderable.width,
                            renderable.height,
                        ) && data.input_region_contains(
                            surface_x,
                            surface_y,
                            renderable.width,
                            renderable.height,
                        );
                        match (run_start, contained) {
                            (None, true) => run_start = Some(x),
                            (Some(start), false) => {
                                if let Some(rect) = OutputRect::new(
                                    f64::from(origin.0 + start as i32),
                                    f64::from(origin.1 + y as i32),
                                    f64::from(x - start),
                                    1.0,
                                ) {
                                    rows.push(rect);
                                }
                                run_start = None;
                            }
                            _ => {}
                        }
                    }
                    if let Some(start) = run_start
                        && let Some(rect) = OutputRect::new(
                            f64::from(origin.0 + start as i32),
                            f64::from(origin.1 + y as i32),
                            f64::from(renderable.width - start),
                            1.0,
                        )
                    {
                        rows.push(rect);
                    }
                }
                rows
            })
            .unwrap_or_default();
        if input_region.is_empty() {
            None
        } else {
            Some(OutputRegion {
                rects: coalesce_output_row_rects(input_region),
            })
        }
    }

    fn pointer_constraint_backend_activated(&mut self, id: PointerConstraintBackendId) {
        if self.pending_backend_constraint != Some(id) {
            pointer_debug_log(format!(
                "backend activated stale id={:?} current_active={:?} current_pending={:?}",
                id, self.active_backend_constraint, self.pending_backend_constraint
            ));
            self.pending_locked_activation_anchors.remove(&id);
            return;
        }
        let locked_activation_anchor = self.pending_locked_activation_anchors.remove(&id);
        let activation = {
            let Some(constraint) = self.pointer_constraints.get_mut(&id.constraint_id) else {
                return;
            };
            if constraint.generation != id.generation || constraint.defunct {
                return;
            }
            constraint.backend_pending = false;
            if constraint.active {
                return;
            }
            constraint.active = true;
            self.pending_backend_constraint = None;
            self.active_backend_constraint = Some(id);
            Some((
                constraint.id,
                constraint.generation,
                constraint.mode,
                compositor_surface_id(&constraint.surface),
                constraint.surface.clone(),
                constraint.pointer.clone(),
                constraint.locked_resource.clone(),
                constraint.confined_resource.clone(),
            ))
        };
        let Some((
            constraint_id,
            generation,
            mode,
            surface_id,
            surface,
            pointer,
            locked_resource,
            confined_resource,
        )) = activation
        else {
            return;
        };
        self.pointer_constraint.activate(mode, surface_id);
        if mode == PointerConstraintMode::Locked {
            if let Some(pending) = self.pending_locked_pointer_reveal.take() {
                pointer_debug_log(format!(
                    "pointer.unlock transition_cancel id={} generation={} reason=new_lock",
                    pending.backend_id.constraint_id, pending.backend_id.generation
                ));
            }
            let activation_anchor = locked_activation_anchor.unwrap_or(OutputPosition {
                x: self.last_pointer_x,
                y: self.last_pointer_y,
            });
            pointer_debug_log(format!(
                "pointer.constraint backend_activated id={} generation={} mode={:?} surface={} pointer={} client={} anchor_output=({},{})",
                id.constraint_id,
                id.generation,
                mode,
                surface_id,
                pointer.id().protocol_id(),
                wayland_resource_client_label(&pointer),
                activation_anchor.x,
                activation_anchor.y
            ));
            self.cursor_visibility.lock_hidden_constraint_id = Some(constraint_id);
            if self.active_client_cursor_has_content() {
                self.advance_render_generation(RenderGenerationCause::CursorState);
            }
            self.sync_cursor_visibility_request();
            let (surface_x, surface_y) = self
                .pointer_target_at(activation_anchor.x, activation_anchor.y)
                .filter(|target| same_surface_resource(&target.surface, &surface))
                .map(|target| (target.surface_x, target.surface_y))
                .unwrap_or((0.0, 0.0));
            self.ensure_pointer_focus(&surface);
            if !self.pointer_resource_entered_surface(&pointer, &surface) {
                let target = PointerTarget {
                    surface: surface.clone(),
                    surface_x,
                    surface_y,
                };
                self.send_pointer_enter_to_resource(&pointer, &target);
            }
            pointer_debug_log(format!(
                "pointer.lock route_active id={} generation={} surface={} pointer={} anchor_output=({},{}) anchor_local=({},{})",
                constraint_id,
                generation,
                compositor_surface_id(&surface),
                pointer.id().protocol_id(),
                activation_anchor.x,
                activation_anchor.y,
                surface_x,
                surface_y
            ));
            self.active_locked_pointer_routing = Some(ActiveLockedPointerRouting {
                constraint_id,
                generation,
                pointer,
                surface,
                surface_x,
                surface_y,
                activation_anchor,
            });
        } else {
            pointer_debug_log(format!(
                "pointer.constraint backend_activated id={} generation={} mode={:?} surface={} pointer={} client={} cursor_output=({},{})",
                id.constraint_id,
                id.generation,
                mode,
                surface_id,
                pointer.id().protocol_id(),
                wayland_resource_client_label(&pointer),
                self.last_pointer_x,
                self.last_pointer_y
            ));
            if mode == PointerConstraintMode::Confined
                && let Some(region) = self.pointer_constraint_output_region(constraint_id)
            {
                let clamped = region.closest_point(OutputPosition {
                    x: self.last_pointer_x,
                    y: self.last_pointer_y,
                });
                self.update_pointer_position(clamped.x, clamped.y);
                let target = self
                    .pointer_target_for_surface_at_output(&surface, clamped.x, clamped.y)
                    .unwrap_or(PointerTarget {
                        surface: surface.clone(),
                        surface_x: 0.0,
                        surface_y: 0.0,
                    });
                self.ensure_pointer_focus(&surface);
                if !self.pointer_resource_entered_surface(&pointer, &surface) {
                    self.send_pointer_enter_to_resource(&pointer, &target);
                }
                pointer_debug_log(format!(
                    "confined route activate id={} surface={} region={:?}",
                    constraint_id,
                    compositor_surface_id(&surface),
                    region.rects
                ));
                self.active_confined_pointer_routing = Some(ActiveConfinedPointerRouting {
                    constraint_id,
                    generation,
                    pointer,
                    surface,
                    region,
                });
            }
        }
        match mode {
            PointerConstraintMode::Locked => {
                if let Some(resource) = &locked_resource {
                    resource.locked();
                }
            }
            PointerConstraintMode::Confined => {
                if let Some(resource) = &confined_resource {
                    resource.confined();
                }
            }
            PointerConstraintMode::None => {}
        }
    }

    fn pointer_constraint_backend_activation_current(
        &self,
        id: PointerConstraintBackendId,
    ) -> bool {
        self.pending_backend_constraint == Some(id)
            && self
                .pointer_constraints
                .get(&id.constraint_id)
                .is_some_and(|constraint| {
                    constraint.generation == id.generation
                        && constraint.backend_pending
                        && !constraint.active
                        && !constraint.defunct
                })
    }

    fn pointer_constraint_backend_failed(&mut self, id: PointerConstraintBackendId, _reason: &str) {
        if self.pending_backend_constraint == Some(id) {
            self.pending_backend_constraint = None;
        }
        self.pending_locked_activation_anchors.remove(&id);
        self.cancel_pending_locked_pointer_reveal_for_id(id, "backend_failed");
        let Some(constraint) = self.pointer_constraints.get_mut(&id.constraint_id) else {
            return;
        };
        if constraint.generation != id.generation {
            return;
        }
        constraint.backend_pending = false;
        if constraint.lifetime == PointerConstraintLifetime::Oneshot {
            constraint.defunct = true;
        }
    }

    fn pointer_constraint_backend_deactivated(&mut self, id: PointerConstraintBackendId) {
        if self.active_backend_constraint == Some(id) {
            self.active_backend_constraint = None;
        }
        self.deactivate_pointer_constraint_by_id(id.constraint_id, true, true, false);
    }

    fn cancel_pending_pointer_constraint_backend_requests(
        &mut self,
        id: PointerConstraintBackendId,
    ) {
        let before = self.pending_pointer_constraint_backend_requests.len();
        self.pending_pointer_constraint_backend_requests
            .retain(|request| {
                !matches!(
                    request,
                    PointerConstraintBackendRequest::ActivateLocked { id: request_id, .. }
                        | PointerConstraintBackendRequest::ActivateConfined {
                            id: request_id,
                            ..
                        }
                        | PointerConstraintBackendRequest::UpdateConfinedRegion {
                            id: request_id,
                            ..
                        } if *request_id == id
                )
            });
        let removed = before - self.pending_pointer_constraint_backend_requests.len();
        if removed > 0 {
            pointer_debug_log(format!(
                "queued activation removed id={} generation={} count={}",
                id.constraint_id, id.generation, removed
            ));
        }
        self.pending_locked_activation_anchors.remove(&id);
        self.cancel_pending_locked_pointer_reveal_for_id(id, "constraint_backend_work_canceled");
        if self.pending_backend_constraint == Some(id) {
            self.pending_backend_constraint = None;
        }
    }

    fn deactivate_pointer_constraint_by_id(
        &mut self,
        constraint_id: u64,
        compositor_driven: bool,
        emit_event: bool,
        queue_backend_deactivate: bool,
    ) {
        let Some((
            was_active,
            was_pending,
            backend_id,
            mode,
            lifetime,
            surface,
            pointer,
            locked_resource,
            confined_resource,
            cursor_position_hint,
        )) = ({
            let Some(constraint) = self.pointer_constraints.get_mut(&constraint_id) else {
                return;
            };
            let was_active = constraint.active;
            let was_pending = constraint.backend_pending;
            let backend_id = constraint.backend_id();
            let mode = constraint.mode;
            let lifetime = constraint.lifetime;
            let surface = constraint.surface.clone();
            let pointer = constraint.pointer.clone();
            let locked_resource = constraint.locked_resource.clone();
            let confined_resource = constraint.confined_resource.clone();
            let cursor_position_hint = constraint.committed_cursor_position_hint;
            pointer_debug_log(format!(
                "pointer.unlock request id={} generation={} mode={:?} active={} pending={}",
                constraint.id, constraint.generation, constraint.mode, was_active, was_pending
            ));
            constraint.active = false;
            constraint.backend_pending = false;
            if compositor_driven && constraint.lifetime == PointerConstraintLifetime::Oneshot {
                constraint.defunct = true;
            }
            Some((
                was_active,
                was_pending,
                backend_id,
                mode,
                lifetime,
                surface,
                pointer,
                locked_resource,
                confined_resource,
                cursor_position_hint,
            ))
        })
        else {
            return;
        };
        if was_pending {
            self.cancel_pending_pointer_constraint_backend_requests(backend_id);
        } else if self.pending_backend_constraint == Some(backend_id) {
            self.pending_backend_constraint = None;
        }
        if self.active_backend_constraint == Some(backend_id) {
            self.active_backend_constraint = None;
        }
        let restore_position = if self
            .active_locked_pointer_routing
            .as_ref()
            .is_some_and(|active| active.constraint_id == constraint_id)
        {
            let restore_position = if mode == PointerConstraintMode::Locked {
                self.restore_locked_pointer_position(&surface, cursor_position_hint)
            } else {
                None
            };
            if mode == PointerConstraintMode::Locked {
                self.clear_active_locked_pointer_routing();
                self.refresh_pointer_focus_at_last_position();
            }
            restore_position
        } else if self
            .active_confined_pointer_routing
            .as_ref()
            .is_some_and(|active| active.constraint_id == constraint_id)
        {
            pointer_debug_log(format!(
                "confined route deactivate id={} reason=constraint_deactivate",
                constraint_id
            ));
            self.clear_active_confined_pointer_routing();
            self.refresh_pointer_focus_at_last_position();
            None
        } else {
            None
        };
        if was_active {
            self.clear_pointer_constraint();
            let locked_unlock_transition = mode == PointerConstraintMode::Locked
                && self.cursor_visibility.lock_hidden_constraint_id == Some(constraint_id);
            if queue_backend_deactivate {
                pointer_debug_log(format!(
                    "backend deactivate queued id={} generation={} reason=constraint_deactivate",
                    backend_id.constraint_id, backend_id.generation
                ));
                self.pending_pointer_constraint_backend_requests.push(
                    PointerConstraintBackendRequest::Deactivate {
                        id: backend_id,
                        restore_position,
                    },
                );
            }
            if emit_event {
                match mode {
                    PointerConstraintMode::Locked => {
                        if let Some(resource) = &locked_resource {
                            resource.unlocked();
                        }
                    }
                    PointerConstraintMode::Confined => {
                        if let Some(resource) = &confined_resource {
                            resource.unconfined();
                        }
                    }
                    PointerConstraintMode::None => {}
                }
            }
            if locked_unlock_transition && pointer.is_alive() && surface.is_alive() {
                self.begin_pending_locked_pointer_reveal(
                    backend_id,
                    pointer,
                    surface.clone(),
                    restore_position,
                );
            }
        } else if was_pending {
            pointer_debug_log(format!(
                "constraint pending activation canceled id={} generation={}",
                backend_id.constraint_id, backend_id.generation
            ));
            if mode == PointerConstraintMode::Locked
                && lifetime == PointerConstraintLifetime::Oneshot
                && let Some(position) =
                    self.valid_cursor_hint_output_position(&surface, cursor_position_hint)
            {
                pointer_debug_log(format!(
                    "oneshot compatibility warp selected id={} generation={} output=({},{})",
                    backend_id.constraint_id, backend_id.generation, position.x, position.y
                ));
                self.apply_pointer_warp(position, true);
            } else if mode == PointerConstraintMode::Locked
                && lifetime == PointerConstraintLifetime::Oneshot
            {
                pointer_debug_log(format!(
                    "oneshot compatibility warp rejected id={} generation={} reason=no_valid_committed_hint",
                    backend_id.constraint_id, backend_id.generation
                ));
            }
        }
        if self.cursor_visibility.lock_hidden_constraint_id == Some(constraint_id)
            && self
                .pending_locked_pointer_reveal
                .as_ref()
                .is_none_or(|pending| pending.backend_id.constraint_id != constraint_id)
        {
            self.cursor_visibility.lock_hidden_constraint_id = None;
            if self.active_client_cursor_has_content() {
                self.advance_render_generation(RenderGenerationCause::CursorState);
            }
            self.sync_cursor_visibility_request();
        }
    }

    fn valid_cursor_hint_output_position(
        &mut self,
        surface: &wl_surface::WlSurface,
        cursor_position_hint: Option<(f64, f64)>,
    ) -> Option<OutputPosition> {
        let (surface_x, surface_y) = cursor_position_hint?;
        if !surface_x.is_finite() || !surface_y.is_finite() {
            pointer_debug_log(format!(
                "pointer cursor_hint ignored reason=non_finite hint=({},{})",
                surface_x, surface_y
            ));
            return None;
        }
        let (x, y) = self.output_position_for_valid_cursor_hint(surface, surface_x, surface_y)?;
        Some(OutputPosition { x, y })
    }

    fn apply_pointer_warp(&mut self, position: OutputPosition, send_motion: bool) {
        let before = OutputPosition {
            x: self.last_pointer_x,
            y: self.last_pointer_y,
        };
        self.update_pointer_position(position.x, position.y);
        pointer_debug_log(format!(
            "pointer warp compositor before=({},{}) after=({},{}) send_motion={}",
            before.x, before.y, position.x, position.y, send_motion
        ));
        self.pending_pointer_constraint_backend_requests
            .push(PointerConstraintBackendRequest::WarpPointer { position });
        if send_motion {
            self.send_pointer_motion_after_warp(position);
        }
    }

    fn send_pointer_motion_after_warp(&mut self, position: OutputPosition) {
        if self.active_locked_pointer_binding().is_some() {
            pointer_debug_log("pointer warp motion suppressed reason=active_lock");
            return;
        }
        if let Some(active) = self.active_confined_pointer_binding() {
            self.pin_confined_pointer_focus(&active);
            return;
        }
        if self.send_implicit_pointer_grab_motion(position.x, position.y) {
            return;
        }
        let Some(target) = self.pointer_target_at(position.x, position.y) else {
            self.clear_pointer_focus();
            return;
        };
        self.ensure_pointer_focus(&target.surface);
        let time = wayland_event_time();
        for pointer in self
            .pointer_resources
            .iter()
            .filter(|pointer| resource_belongs_to_surface_client(*pointer, &target.surface))
        {
            let _ = pointer.send_event(wl_pointer::Event::Motion {
                time,
                surface_x: target.surface_x,
                surface_y: target.surface_y,
            });
            send_pointer_frame_if_supported(pointer);
        }
    }

    fn remove_pointer_constraint(&mut self, constraint_id: u64) {
        self.cancel_pending_locked_pointer_reveal_for_constraint(
            constraint_id,
            "constraint_removed",
        );
        let was_active = self
            .pointer_constraints
            .get(&constraint_id)
            .is_some_and(|constraint| constraint.active || constraint.backend_pending);
        if was_active {
            if let Some(constraint) = self.pointer_constraints.get_mut(&constraint_id) {
                constraint.defunct = true;
            }
            self.deactivate_pointer_constraint_by_id(constraint_id, false, false, true);
        }
        self.pointer_constraints.remove(&constraint_id);
        if self.cursor_visibility.lock_hidden_constraint_id == Some(constraint_id)
            && self
                .pending_locked_pointer_reveal
                .as_ref()
                .is_none_or(|pending| pending.backend_id.constraint_id != constraint_id)
        {
            self.cursor_visibility.lock_hidden_constraint_id = None;
            if self.active_client_cursor_has_content() {
                self.advance_render_generation(RenderGenerationCause::CursorState);
            }
            self.sync_cursor_visibility_request();
        }
    }

    fn deactivate_pointer_constraints_for_pointer(
        &mut self,
        pointer: &wl_pointer::WlPointer,
        emit_event: bool,
    ) {
        let ids = self
            .pointer_constraints
            .values()
            .filter(|constraint| same_wayland_resource(&constraint.pointer, pointer))
            .map(|constraint| constraint.id)
            .collect::<Vec<_>>();
        for id in ids {
            self.cancel_pending_locked_pointer_reveal_for_constraint(id, "pointer_destroyed");
            if let Some(constraint) = self.pointer_constraints.get_mut(&id) {
                constraint.defunct = true;
            }
            self.deactivate_pointer_constraint_by_id(id, true, emit_event, true);
            self.pointer_constraints.remove(&id);
        }
    }

    fn deactivate_pointer_constraints_for_surface(&mut self, surface_id: u32, emit_event: bool) {
        let ids = self
            .pointer_constraints
            .values()
            .filter(|constraint| compositor_surface_id(&constraint.surface) == surface_id)
            .map(|constraint| constraint.id)
            .collect::<Vec<_>>();
        for id in ids {
            self.cancel_pending_locked_pointer_reveal_for_constraint(id, "surface_destroyed");
            if let Some(constraint) = self.pointer_constraints.get_mut(&id) {
                constraint.defunct = true;
            }
            self.deactivate_pointer_constraint_by_id(id, true, emit_event, true);
            self.pointer_constraints.remove(&id);
        }
    }

    fn deactivate_pointer_constraints_for_surface_focus_loss(
        &mut self,
        surface_id: u32,
        emit_event: bool,
    ) {
        let ids = self
            .pointer_constraints
            .values()
            .filter(|constraint| compositor_surface_id(&constraint.surface) == surface_id)
            .map(|constraint| constraint.id)
            .collect::<Vec<_>>();
        for id in ids {
            self.deactivate_pointer_constraint_by_id(id, true, emit_event, true);
        }
    }

    fn set_pointer_constraint_pending_region(
        &mut self,
        constraint_id: u64,
        region: SurfaceInputRegion,
    ) {
        if let Some(constraint) = self.pointer_constraints.get_mut(&constraint_id) {
            constraint.pending_region = region;
        }
    }

    fn set_pointer_constraint_pending_cursor_position_hint(
        &mut self,
        constraint_id: u64,
        surface_x: f64,
        surface_y: f64,
    ) {
        if !surface_x.is_finite() || !surface_y.is_finite() {
            pointer_debug_log(format!(
                "pointer.lock cursor_hint ignored id={} reason=non_finite hint=({},{})",
                constraint_id, surface_x, surface_y
            ));
            return;
        }
        if let Some(constraint) = self.pointer_constraints.get_mut(&constraint_id) {
            constraint.pending_cursor_position_hint = Some((surface_x, surface_y));
        }
    }

    fn apply_pending_pointer_constraint_state_for_surface(&mut self, surface_id: u32) {
        let ids = self
            .pointer_constraints
            .values()
            .filter(|constraint| compositor_surface_id(&constraint.surface) == surface_id)
            .map(|constraint| constraint.id)
            .collect::<Vec<_>>();
        for id in ids {
            if let Some(constraint) = self.pointer_constraints.get_mut(&id) {
                constraint.committed_region = constraint.pending_region.clone();
                constraint.committed_cursor_position_hint = constraint.pending_cursor_position_hint;
            }
            self.update_active_confined_pointer_region(id, "commit");
            self.maybe_request_pointer_constraint_activation(id);
        }
    }

    fn update_active_confined_pointer_region(&mut self, constraint_id: u64, reason: &'static str) {
        let Some(active) = self.active_confined_pointer_binding() else {
            return;
        };
        if active.constraint_id != constraint_id {
            return;
        }
        let Some(region) = self.pointer_constraint_output_region(constraint_id) else {
            return;
        };
        if region == active.region {
            return;
        }
        pointer_debug_log(format!(
            "confined route update id={} old={:?} new={:?} reason={}",
            constraint_id, active.region.rects, region.rects, reason
        ));
        let id = PointerConstraintBackendId {
            constraint_id,
            generation: active.generation,
        };
        self.pending_pointer_constraint_backend_requests.push(
            PointerConstraintBackendRequest::UpdateConfinedRegion {
                id,
                region: region.clone(),
            },
        );
        self.active_confined_pointer_routing = Some(ActiveConfinedPointerRouting {
            region: region.clone(),
            ..active
        });
        let position = OutputPosition {
            x: self.last_pointer_x,
            y: self.last_pointer_y,
        };
        if region.closest_point(position) != position {
            self.send_confined_pointer_motion(position.x, position.y);
        }
    }

    fn update_all_active_confined_pointer_regions(&mut self, reason: &'static str) {
        let Some(active) = self.active_confined_pointer_binding() else {
            return;
        };
        self.update_active_confined_pointer_region(active.constraint_id, reason);
    }

    fn take_pointer_constraint_backend_requests(&mut self) -> Vec<PointerConstraintBackendRequest> {
        std::mem::take(&mut self.pending_pointer_constraint_backend_requests)
    }

    fn add_idle_inhibitor(&mut self, inhibitor: zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1) {
        self.idle_inhibitor_resources.push(inhibitor);
        self.idle_manager.inhibit();
    }

    fn remove_idle_inhibitor(&mut self, inhibitor: &zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1) {
        let before = self.idle_inhibitor_resources.len();
        self.idle_inhibitor_resources
            .retain(|resource| !same_wayland_resource(resource, inhibitor));
        if self.idle_inhibitor_resources.len() != before {
            self.idle_manager.uninhibit();
        }
    }

    pub fn idle_inhibited(&mut self) -> bool {
        self.idle_inhibitor_resources.retain(Resource::is_alive);
        if self.idle_inhibitor_resources.is_empty() {
            while self.idle_manager.is_inhibited() {
                self.idle_manager.uninhibit();
            }
        }
        self.idle_manager.is_inhibited()
    }

    fn add_relative_pointer_resource(
        &mut self,
        pointer: zwp_relative_pointer_v1::ZwpRelativePointerV1,
        source_pointer: wl_pointer::WlPointer,
    ) {
        pointer_debug_log(format!(
            "pointer.relative create relative={} source_pointer={} client={}",
            pointer.id().protocol_id(),
            source_pointer.id().protocol_id(),
            wayland_resource_client_label(&source_pointer)
        ));
        self.relative_pointer_resources
            .push(RelativePointerResource {
                resource: pointer,
                source_pointer,
            });
    }

    fn remove_relative_pointer_resource(
        &mut self,
        pointer: &zwp_relative_pointer_v1::ZwpRelativePointerV1,
    ) {
        pointer_debug_log(format!(
            "pointer.relative destroy relative={} client={}",
            pointer.id().protocol_id(),
            wayland_resource_client_label(pointer)
        ));
        self.relative_pointer_resources
            .retain(|resource| !same_wayland_resource(&resource.resource, pointer));
    }

    fn send_relative_pointer_motion(&mut self, timestamp_usec: u64, motion: RelativePointerMotion) {
        if motion.is_zero() {
            return;
        }
        self.relative_pointer_resources
            .retain(|resource| resource.resource.is_alive() && resource.source_pointer.is_alive());
        let live_relative_count = self.relative_pointer_resources.len();
        if let Some(active) = self.active_locked_pointer_binding() {
            self.pin_locked_pointer_focus(&active);
            self.dispatch_locked_relative_pointer_motion(
                timestamp_usec,
                motion,
                &active,
                live_relative_count,
            );
            return;
        }

        let Some(surface) = self.pointer_surface.clone() else {
            self.relative_motion_debug.note_drop(format!(
                "no pointer focus; active_lock=absent relative_resources={live_relative_count}"
            ));
            return;
        };
        let dispatch_count = self.dispatch_relative_pointer_motion_to_surface_client(
            timestamp_usec,
            motion,
            &surface,
        );
        if dispatch_count == 0 {
            self.relative_motion_debug.note_drop(format!(
                "unlocked route found no recipient; pointer_surface={} client={} relative_resources={live_relative_count}",
                compositor_surface_id(&surface),
                wayland_resource_client_label(&surface)
            ));
        }
    }

    fn dispatch_locked_relative_pointer_motion(
        &mut self,
        timestamp_usec: u64,
        motion: RelativePointerMotion,
        active: &ActiveLockedPointerRouting,
        live_relative_count: usize,
    ) {
        let utime_hi = (timestamp_usec >> 32) as u32;
        let utime_lo = (timestamp_usec & 0xffff_ffff) as u32;
        let pointer_entered =
            self.pointer_resource_entered_surface(&active.pointer, &active.surface);
        let relative_pointers = self.relative_pointer_resources.clone();
        let mut recipients: Vec<RelativePointerResource> = Vec::new();
        let mut exact_source_pointer_count = 0usize;
        let mut same_client_count = 0usize;
        let mut same_seat_count = 0usize;
        let mut stale_count = 0usize;
        let mut cross_client_count = 0usize;

        for relative_pointer in relative_pointers {
            if !relative_pointer.resource.is_alive() || !relative_pointer.source_pointer.is_alive()
            {
                stale_count += 1;
                continue;
            }
            if !resource_belongs_to_surface_client(&relative_pointer.resource, &active.surface)
                || !resource_belongs_to_surface_client(
                    &relative_pointer.source_pointer,
                    &active.surface,
                )
            {
                cross_client_count += 1;
                continue;
            }
            same_client_count += 1;
            // Typhon currently exposes a single wl_seat. Exact wl_pointer
            // resource equality is too strict because clients may create
            // constraints and relative-pointer resources from different
            // wl_pointer objects on the same client seat. When multi-seat
            // support is added, store and compare an explicit seat id here.
            same_seat_count += 1;
            if same_wayland_resource(&relative_pointer.source_pointer, &active.pointer) {
                exact_source_pointer_count += 1;
            }
            if !recipients.iter().any(|recipient| {
                same_wayland_resource(&recipient.resource, &relative_pointer.resource)
            }) {
                recipients.push(relative_pointer);
            }
        }

        let selected_recipient_count = recipients.len();

        if self.relative_motion_debug.should_log_route_snapshot() {
            let relative_sources = self
                .relative_pointer_resources
                .iter()
                .map(|relative_pointer| {
                    format!(
                        "relative={} source_pointer={} source_client={} source_seat=untracked",
                        relative_pointer.resource.id().protocol_id(),
                        relative_pointer.source_pointer.id().protocol_id(),
                        wayland_resource_client_label(&relative_pointer.source_pointer)
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            pointer_debug_log(format!(
                "relative route snapshot constraint={} generation={} surface={} surface_client={} lock_pointer={} lock_client={} lock_seat=single exact_source_pointer_count={} same_client_count={} same_seat_count={} selected_recipient_count={} pointer_entered={} live_relative_count={} stale_count={} cross_client_count={} [{}]",
                active.constraint_id,
                active.generation,
                compositor_surface_id(&active.surface),
                wayland_resource_client_label(&active.surface),
                active.pointer.id().protocol_id(),
                wayland_resource_client_label(&active.pointer),
                exact_source_pointer_count,
                same_client_count,
                same_seat_count,
                selected_recipient_count,
                pointer_entered,
                live_relative_count,
                stale_count,
                cross_client_count,
                relative_sources
            ));
        }

        let dispatched_ids = recipients
            .iter()
            .map(|relative_pointer| relative_pointer.resource.id().protocol_id())
            .collect::<Vec<_>>();
        pointer_debug_log(format!(
            "relative route exact_source_pointer_count={} same_client_count={} same_seat_count={} selected_recipient_count={} dispatched={:?} client={} seat=single constraint={} generation={}",
            exact_source_pointer_count,
            same_client_count,
            same_seat_count,
            selected_recipient_count,
            dispatched_ids,
            wayland_resource_client_label(&active.surface),
            active.constraint_id,
            active.generation
        ));

        let mut frame_pointers: Vec<wl_pointer::WlPointer> = Vec::new();
        let mut relative_events_sent = 0usize;
        for relative_pointer in recipients {
            relative_pointer.resource.relative_motion(
                utime_hi,
                utime_lo,
                motion.dx,
                motion.dy,
                motion.dx_unaccelerated,
                motion.dy_unaccelerated,
            );
            relative_events_sent += 1;
            if relative_pointer.source_pointer.is_alive()
                && !frame_pointers
                    .iter()
                    .any(|pointer| same_wayland_resource(pointer, &relative_pointer.source_pointer))
            {
                frame_pointers.push(relative_pointer.source_pointer.clone());
            }
            self.relative_motion_debug.note_dispatch(format!(
                "relative motion dispatched constraint={} generation={} pointer={} source_pointer={} relative={} dx={} dy={}",
                active.constraint_id,
                active.generation,
                active.pointer.id().protocol_id(),
                relative_pointer.source_pointer.id().protocol_id(),
                relative_pointer.resource.id().protocol_id(),
                motion.dx,
                motion.dy
            ));
        }
        let source_pointer_ids = frame_pointers
            .iter()
            .map(|pointer| pointer.id().protocol_id())
            .collect::<Vec<_>>();
        let unique_source_pointer_count = frame_pointers.len();
        let mut pointer_frames_sent = 0usize;
        for pointer in frame_pointers {
            if pointer.is_alive() {
                send_pointer_frame_if_supported(&pointer);
                pointer_frames_sent += 1;
            }
        }
        pointer_debug_log(format!(
            "pointer.relative locked_dispatch constraint={} generation={} selected_recipient_count={} unique_source_pointer_count={} relative_events_sent={} pointer_frames_sent={} source_pointers={:?}",
            active.constraint_id,
            active.generation,
            selected_recipient_count,
            unique_source_pointer_count,
            relative_events_sent,
            pointer_frames_sent,
            source_pointer_ids
        ));
        if relative_events_sent == 0 {
            let reason = if same_client_count > 0 {
                format!(
                    "locked route rejected all same-client relative pointers; constraint={} generation={} pointer={} client={} surface={} client={} exact_source_pointer_count={} same_client_count={} same_seat_count={} selected_recipient_count={} stale_count={} cross_client_count={} pointer_entered={pointer_entered} relative_resources={live_relative_count}",
                    active.constraint_id,
                    active.generation,
                    active.pointer.id().protocol_id(),
                    wayland_resource_client_label(&active.pointer),
                    compositor_surface_id(&active.surface),
                    wayland_resource_client_label(&active.surface),
                    exact_source_pointer_count,
                    same_client_count,
                    same_seat_count,
                    selected_recipient_count,
                    stale_count,
                    cross_client_count,
                )
            } else {
                format!(
                    "locked route has no same-client relative pointer; constraint={} generation={} pointer={} client={} surface={} client={} exact_source_pointer_count={} same_client_count=0 same_seat_count=0 selected_recipient_count=0 stale_count={} cross_client_count={} pointer_entered={pointer_entered} relative_resources={live_relative_count}",
                    active.constraint_id,
                    active.generation,
                    active.pointer.id().protocol_id(),
                    wayland_resource_client_label(&active.pointer),
                    compositor_surface_id(&active.surface),
                    wayland_resource_client_label(&active.surface),
                    exact_source_pointer_count,
                    stale_count,
                    cross_client_count,
                )
            };
            self.relative_motion_debug.note_drop(reason);
        }
    }

    fn dispatch_relative_pointer_motion_to_surface_client(
        &mut self,
        timestamp_usec: u64,
        motion: RelativePointerMotion,
        surface: &wl_surface::WlSurface,
    ) -> usize {
        let utime_hi = (timestamp_usec >> 32) as u32;
        let utime_lo = (timestamp_usec & 0xffff_ffff) as u32;
        let relative_pointers = self.relative_pointer_resources.clone();
        let mut dispatched_resource_ids = HashSet::new();
        for relative_pointer in relative_pointers {
            if !relative_pointer.resource.is_alive() || !relative_pointer.source_pointer.is_alive()
            {
                continue;
            }
            if !resource_belongs_to_surface_client(&relative_pointer.resource, surface) {
                continue;
            }
            let resource_id = relative_pointer.resource.id().protocol_id();
            if !dispatched_resource_ids.insert(resource_id) {
                continue;
            }
            relative_pointer.resource.relative_motion(
                utime_hi,
                utime_lo,
                motion.dx,
                motion.dy,
                motion.dx_unaccelerated,
                motion.dy_unaccelerated,
            );
            self.relative_motion_debug.note_dispatch(format!(
                "relative motion dispatched client={} relative={} dx={} dy={}",
                wayland_resource_client_label(surface),
                resource_id,
                motion.dx,
                motion.dy
            ));
        }
        dispatched_resource_ids.len()
    }

    fn remember_held_pointer_button(&mut self, press: PointerPress) {
        if self
            .held_pointer_buttons
            .iter()
            .any(|held| held.button == press.button)
        {
            pointer_debug_log(format!(
                "duplicate button press ignored button={}",
                press.button
            ));
            return;
        }
        pointer_debug_log(format!(
            "button press button={} surface={} held_count={}",
            press.button,
            compositor_surface_id(&press.surface),
            self.held_pointer_buttons.len() + 1
        ));
        self.held_pointer_buttons.push(press);
    }

    fn forget_held_pointer_button(&mut self, button: u32) {
        let before = self.held_pointer_buttons.len();
        self.held_pointer_buttons
            .retain(|held| held.button != button);
        if before == self.held_pointer_buttons.len() {
            pointer_debug_log(format!("unmatched button release ignored button={button}"));
        } else {
            pointer_debug_log(format!(
                "button release button={} held_count={}",
                button,
                self.held_pointer_buttons.len()
            ));
        }
    }

    fn implicit_pointer_grab_surface(
        &mut self,
        reason: &'static str,
    ) -> Option<wl_surface::WlSurface> {
        let grab = self.implicit_pointer_grab.clone()?;
        let surface_id = compositor_surface_id(&grab.surface);
        let mapped = self
            .renderable_surfaces
            .iter()
            .any(|renderable| renderable.surface_id == surface_id);
        if !grab.surface.is_alive() || !mapped {
            self.cancel_implicit_pointer_grab_for_surface_ids(&[surface_id], reason);
            return None;
        }
        Some(grab.surface)
    }

    fn begin_implicit_pointer_grab(&mut self, press: &PointerPress) {
        if self.implicit_pointer_grab.is_some() {
            return;
        }
        self.implicit_pointer_grab = Some(ImplicitPointerGrab {
            surface: press.surface.clone(),
            root_surface_id: press.root_surface_id,
        });
        pointer_debug_log(format!(
            "implicit grab begin surface={} button={}",
            compositor_surface_id(&press.surface),
            press.button
        ));
    }

    fn end_implicit_pointer_grab(&mut self, reason: &'static str) {
        let Some(grab) = self.implicit_pointer_grab.take() else {
            return;
        };
        pointer_debug_log(format!(
            "implicit grab end surface={} reason={}",
            compositor_surface_id(&grab.surface),
            reason
        ));
    }

    fn cancel_implicit_pointer_grab_for_surface_ids(
        &mut self,
        surface_ids: &[u32],
        reason: &'static str,
    ) {
        let Some(grab) = self.implicit_pointer_grab.as_ref() else {
            return;
        };
        let grab_surface_id = compositor_surface_id(&grab.surface);
        if !surface_ids.contains(&grab_surface_id) && !surface_ids.contains(&grab.root_surface_id) {
            return;
        }
        self.end_implicit_pointer_grab(reason);
        self.held_pointer_buttons.retain(|press| {
            !surface_ids.contains(&compositor_surface_id(&press.surface))
                && !surface_ids.contains(&press.root_surface_id)
        });
        if self.last_pointer_press.as_ref().is_some_and(|press| {
            surface_ids.contains(&compositor_surface_id(&press.surface))
                || surface_ids.contains(&press.root_surface_id)
        }) {
            self.last_pointer_press = None;
        }
    }

    fn pointer_target_for_grabbed_surface_at_output(
        &mut self,
        surface: &wl_surface::WlSurface,
        x: f64,
        y: f64,
    ) -> Option<PointerTarget> {
        let surface_id = compositor_surface_id(surface);
        self.refresh_surface_origin_cache();
        let index = self
            .renderable_surfaces
            .iter()
            .position(|renderable| renderable.surface_id == surface_id)?;
        let origin = self.surface_origin_cache.get(index).copied()?;
        Some(PointerTarget {
            surface: surface.clone(),
            surface_x: x - f64::from(origin.0),
            surface_y: y - f64::from(origin.1),
        })
    }

    fn send_implicit_pointer_grab_motion(&mut self, x: f64, y: f64) -> bool {
        let Some(surface) = self.implicit_pointer_grab_surface("surface-destroyed") else {
            return false;
        };
        let Some(target) = self.pointer_target_for_grabbed_surface_at_output(&surface, x, y) else {
            let surface_id = compositor_surface_id(&surface);
            self.cancel_implicit_pointer_grab_for_surface_ids(&[surface_id], "surface-destroyed");
            self.refresh_pointer_focus_at_last_position();
            return true;
        };
        pointer_debug_log(format!(
            "implicit grab motion surface={} output=({},{}) local=({},{})",
            compositor_surface_id(&surface),
            x,
            y,
            target.surface_x,
            target.surface_y
        ));
        let time = wayland_event_time();
        for pointer in self
            .pointer_resources
            .iter()
            .filter(|pointer| resource_belongs_to_surface_client(*pointer, &surface))
        {
            let _ = pointer.send_event(wl_pointer::Event::Motion {
                time,
                surface_x: target.surface_x,
                surface_y: target.surface_y,
            });
            send_pointer_frame_if_supported(pointer);
        }
        true
    }

    fn send_pointer_button(&mut self, button: u32, pressed: bool) {
        if let Some(locked_surface) = self.locked_pointer_input_surface() {
            self.ensure_pointer_focus(&locked_surface);
            if let Some(active) = self.active_locked_pointer_binding() {
                self.pin_locked_pointer_focus(&active);
            }
            let surface = locked_surface;
            let state = if pressed {
                wl_pointer::ButtonState::Pressed
            } else {
                wl_pointer::ButtonState::Released
            };
            let serial = self.next_configure_serial();
            let time = wayland_event_time();
            self.remember_input_serial(serial, surface.clone());
            if pressed {
                let surface_id = compositor_surface_id(&surface);
                let root_surface_id = self.root_surface_id_for_surface(surface_id);
                if self
                    .topmost_popup_grab_surface_id()
                    .is_some_and(|popup_id| self.surface_is_descendant_of(surface_id, popup_id))
                {
                    self.focus_surface(surface.clone());
                } else if let Some(root_surface) = self.surface_resource_by_id(root_surface_id) {
                    self.focus_surface(root_surface);
                }
                let press = PointerPress {
                    serial,
                    button,
                    surface: surface.clone(),
                    root_surface_id,
                    output_x: self.last_pointer_x,
                    output_y: self.last_pointer_y,
                };
                self.remember_held_pointer_button(press.clone());
                self.last_pointer_press = Some(press);
            } else if self
                .last_pointer_press
                .as_ref()
                .is_some_and(|press| press.button == button)
            {
                self.forget_held_pointer_button(button);
                self.last_pointer_press = None;
            } else {
                self.forget_held_pointer_button(button);
            }
            if !pressed
                && self.held_pointer_buttons.is_empty()
                && self.implicit_pointer_grab.is_some()
            {
                self.end_implicit_pointer_grab("last-release");
            }
            for pointer in self
                .pointer_resources
                .iter()
                .filter(|pointer| resource_belongs_to_surface_client(*pointer, &surface))
            {
                let _ = pointer.send_event(wl_pointer::Event::Button {
                    serial,
                    time,
                    button,
                    state: WEnum::Value(state),
                });
                send_pointer_frame_if_supported(pointer);
            }
            return;
        }

        let grabbed_surface = self.implicit_pointer_grab_surface("surface-destroyed");
        let target = if grabbed_surface.is_none() {
            self.pointer_target_at(self.last_pointer_x, self.last_pointer_y)
        } else {
            None
        };
        if grabbed_surface.is_none() {
            if pressed
                && let Some(popup_surface_id) =
                    self.popup_grab_to_dismiss_for_pointer_target(target.as_ref())
            {
                self.dismiss_popup_surface(popup_surface_id);
                let _ = self.focus_topmost_renderable_toplevel();
                return;
            }

            if let Some(target) = target.as_ref() {
                self.ensure_pointer_focus(&target.surface);
                self.send_pointer_enter_if_needed(target);
            }
        }

        let Some(surface) = grabbed_surface
            .or_else(|| {
                (!pressed).then(|| {
                    self.last_pointer_press
                        .as_ref()
                        .filter(|press| press.button == button)
                        .map(|press| press.surface.clone())
                })?
            })
            .or_else(|| target.map(|target| target.surface))
            .or_else(|| self.pointer_surface.clone())
            .or_else(|| self.focused_surface.clone())
        else {
            return;
        };
        let state = if pressed {
            wl_pointer::ButtonState::Pressed
        } else {
            wl_pointer::ButtonState::Released
        };
        let serial = self.next_configure_serial();
        let time = wayland_event_time();
        self.remember_input_serial(serial, surface.clone());

        if pressed {
            let surface_id = compositor_surface_id(&surface);
            let root_surface_id = self.root_surface_id_for_surface(surface_id);
            if self
                .topmost_popup_grab_surface_id()
                .is_some_and(|popup_id| self.surface_is_descendant_of(surface_id, popup_id))
            {
                self.focus_surface(surface.clone());
            } else if let Some(root_surface) = self.surface_resource_by_id(root_surface_id) {
                self.focus_surface(root_surface);
            }
            let press = PointerPress {
                serial,
                button,
                surface: surface.clone(),
                root_surface_id,
                output_x: self.last_pointer_x,
                output_y: self.last_pointer_y,
            };
            let was_empty = self.held_pointer_buttons.is_empty();
            self.remember_held_pointer_button(press.clone());
            if was_empty
                && self
                    .held_pointer_buttons
                    .iter()
                    .any(|held| held.button == button)
            {
                self.begin_implicit_pointer_grab(&press);
            }
            self.last_pointer_press = Some(press);
        } else if self
            .last_pointer_press
            .as_ref()
            .is_some_and(|press| press.button == button)
        {
            self.forget_held_pointer_button(button);
            self.last_pointer_press = None;
        } else {
            self.forget_held_pointer_button(button);
        }

        for pointer in self
            .pointer_resources
            .iter()
            .filter(|pointer| resource_belongs_to_surface_client(*pointer, &surface))
        {
            let _ = pointer.send_event(wl_pointer::Event::Button {
                serial,
                time,
                button,
                state: WEnum::Value(state),
            });
            send_pointer_frame_if_supported(pointer);
        }
        pointer_debug_log(format!(
            "implicit grab button surface={} button={} state={} held={}",
            compositor_surface_id(&surface),
            button,
            if pressed { "pressed" } else { "released" },
            self.held_pointer_buttons.len()
        ));
        if !pressed && self.held_pointer_buttons.is_empty() && self.implicit_pointer_grab.is_some()
        {
            let old_surface_id = self
                .implicit_pointer_grab
                .as_ref()
                .map(|grab| compositor_surface_id(&grab.surface));
            self.end_implicit_pointer_grab("last-release");
            self.refresh_pointer_focus_after_implicit_grab(old_surface_id);
        }
    }

    fn send_pointer_axis(&mut self, horizontal: f64, vertical: f64) {
        if horizontal == 0.0 && vertical == 0.0 {
            return;
        }

        if let Some(surface) = self.locked_pointer_input_surface() {
            if let Some(active) = self.active_locked_pointer_binding() {
                self.pin_locked_pointer_focus(&active);
            }
            self.ensure_pointer_focus(&surface);
            let time = wayland_event_time();
            for pointer in self
                .pointer_resources
                .iter()
                .filter(|pointer| resource_belongs_to_surface_client(*pointer, &surface))
            {
                if horizontal != 0.0 {
                    let _ = pointer.send_event(wl_pointer::Event::Axis {
                        time,
                        axis: WEnum::Value(wl_pointer::Axis::HorizontalScroll),
                        value: horizontal,
                    });
                }
                if vertical != 0.0 {
                    let _ = pointer.send_event(wl_pointer::Event::Axis {
                        time,
                        axis: WEnum::Value(wl_pointer::Axis::VerticalScroll),
                        value: vertical,
                    });
                }
                send_pointer_frame_if_supported(pointer);
            }
            return;
        }

        if let Some(surface) = self.implicit_pointer_grab_surface("surface-destroyed") {
            let time = wayland_event_time();
            for pointer in self
                .pointer_resources
                .iter()
                .filter(|pointer| resource_belongs_to_surface_client(*pointer, &surface))
            {
                if horizontal != 0.0 {
                    let _ = pointer.send_event(wl_pointer::Event::Axis {
                        time,
                        axis: WEnum::Value(wl_pointer::Axis::HorizontalScroll),
                        value: horizontal,
                    });
                }
                if vertical != 0.0 {
                    let _ = pointer.send_event(wl_pointer::Event::Axis {
                        time,
                        axis: WEnum::Value(wl_pointer::Axis::VerticalScroll),
                        value: vertical,
                    });
                }
                send_pointer_frame_if_supported(pointer);
            }
            return;
        }

        let Some(target) = self.pointer_target_at(self.last_pointer_x, self.last_pointer_y) else {
            self.clear_pointer_focus();
            return;
        };
        if !self.pointer_target_allowed_by_popup_grab(&target) {
            self.clear_pointer_focus();
            return;
        }
        let time = wayland_event_time();
        self.ensure_pointer_focus(&target.surface);
        self.send_pointer_enter_if_needed(&target);

        for pointer in self
            .pointer_resources
            .iter()
            .filter(|pointer| resource_belongs_to_surface_client(*pointer, &target.surface))
        {
            if horizontal != 0.0 {
                let _ = pointer.send_event(wl_pointer::Event::Axis {
                    time,
                    axis: WEnum::Value(wl_pointer::Axis::HorizontalScroll),
                    value: horizontal,
                });
            }
            if vertical != 0.0 {
                let _ = pointer.send_event(wl_pointer::Event::Axis {
                    time,
                    axis: WEnum::Value(wl_pointer::Axis::VerticalScroll),
                    value: vertical,
                });
            }
            send_pointer_frame_if_supported(pointer);
        }
    }

    fn commit_surface_buffer(
        &mut self,
        surface_id: u32,
        pending: PendingSurfaceBuffer,
        damage: RenderableSurfaceDamage,
    ) {
        let resize_commit = pending.resize_commit.as_deref().copied();
        if let Some(surface) = self.surface_resource_by_id(surface_id) {
            self.ensure_surface_entered_outputs(&surface);
        }

        let generation = self.next_render_generation_value();
        let resize_placement = match self.take_pending_resize_commit_placement(surface_id, &pending)
        {
            Ok(placement) => placement,
            Err(_) => return,
        };
        let resize_commit_accepted = resize_placement.is_some();
        let placement = resize_placement.unwrap_or_else(|| self.surface_placement(surface_id));
        self.store_surface_placement(surface_id, placement);
        let buffer_width = match pending.data.width() {
            Ok(width) => width,
            Err(_) => return,
        };
        let buffer_height = match pending.data.height() {
            Ok(height) => height,
            Err(_) => return,
        };
        let Some(buffer_size) = BufferSize::new(buffer_width, buffer_height) else {
            return;
        };
        let surface_size = pending.surface_size.unwrap_or(buffer_size);
        let width = surface_size.width;
        let height = surface_size.height;
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: commit surface={surface_id} wl_buffer={} buffer_id={} buffer={}x{} surface={}x{} offset={},{} shm={} dmabuf={} dmabuf_layout={:?} commit_resize_serial={:?} pending_resize={:?} window_geometry={:?}",
                pending.resource.id().protocol_id(),
                pending.data.buffer_id().get(),
                buffer_width,
                buffer_height,
                width,
                height,
                pending.x,
                pending.y,
                pending.data.is_shm(),
                pending.data.is_dmabuf(),
                pending.data.dmabuf_handle(),
                pending.resize_commit.as_deref().map(|resize| resize.serial),
                resize_commit.map(|resize| resize.serial),
                self.surface_window_geometries.get(&surface_id),
            );
        }
        if let Some(root_surface_id) = self.minimized_root_surface_id_for_surface(surface_id) {
            let damage = damage.normalized_for_surface(buffer_width, buffer_height);
            if self
                .commit_minimized_surface_buffer(
                    root_surface_id,
                    surface_id,
                    &pending,
                    buffer_size,
                    width,
                    height,
                    placement,
                    generation,
                    damage,
                )
                .is_err()
            {
                return;
            }
            self.track_committed_buffer_lifetime(surface_id, &pending);
            self.current_surface_buffers.insert(surface_id, pending);
            if let Some(resize_commit) = resize_commit {
                self.complete_applied_resize_transaction(surface_id, resize_commit);
            }
            return;
        }
        if let Some(existing) = self
            .renderable_surfaces
            .iter_mut()
            .find(|surface| surface.surface_id == surface_id)
        {
            let damage = if existing.buffer_size() == buffer_size
                && existing.buffer_id() == pending.data.buffer_id()
            {
                damage.normalized_for_surface(buffer_width, buffer_height)
            } else {
                RenderableSurfaceDamage::Full
            };
            if update_renderable_surface_buffer(
                existing,
                &pending,
                buffer_size,
                width,
                height,
                placement,
                generation,
                damage,
            )
            .is_err()
            {
                return;
            }
            if resize_commit_accepted {
                existing.width = width;
                existing.height = height;
                existing.placement = placement;
                existing.resize_preview = None;
            }
            let visual_placement = existing.placement;
            self.store_surface_placement(surface_id, visual_placement);
        } else {
            let damage = damage.normalized_for_surface(buffer_width, buffer_height);
            let surface =
                match pending.to_renderable_surface(surface_id, placement, generation, damage) {
                    Ok(surface) => surface,
                    Err(_) => return,
                };
            self.renderable_surfaces.push(surface);
        }
        self.reorder_renderable_surfaces_by_committed_stack();

        let committed_popup = self.popup_surfaces.contains_key(&surface_id);
        if committed_popup {
            if compositor_debug_surface_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: popup surface {surface_id} committed {width}x{height} at buffer offset {},{}",
                    pending.x, pending.y
                );
            }
            self.raise_renderable_surface_tree(surface_id);
        }

        self.track_committed_buffer_lifetime(surface_id, &pending);
        self.current_surface_buffers.insert(surface_id, pending);
        self.set_render_generation(generation, RenderGenerationCause::SurfaceCommit);
        if let Some(resize_commit) = resize_commit {
            self.complete_applied_resize_transaction(surface_id, resize_commit);
        }
        if committed_popup {
            self.refresh_pointer_focus_at_last_position();
        }
    }

    fn minimized_root_surface_id_for_surface(&self, surface_id: u32) -> Option<u32> {
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        self.toplevel_surfaces
            .get(&root_surface_id)
            .is_some_and(|toplevel| toplevel.window.is_minimized())
            .then_some(root_surface_id)
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_minimized_surface_buffer(
        &mut self,
        root_surface_id: u32,
        surface_id: u32,
        pending: &PendingSurfaceBuffer,
        buffer_size: BufferSize,
        width: u32,
        height: u32,
        placement: SurfacePlacement,
        generation: u64,
        damage: RenderableSurfaceDamage,
    ) -> io::Result<()> {
        let renderable_count = self.renderable_surfaces.len();
        self.renderable_surfaces
            .retain(|surface| surface.surface_id != surface_id);
        if self.renderable_surfaces.len() != renderable_count {
            self.invalidate_surface_origin_cache();
        }
        let Some(toplevel) = self.toplevel_surfaces.get_mut(&root_surface_id) else {
            return Ok(());
        };
        if let Some(existing) = toplevel.window.minimized_surface_mut(surface_id) {
            update_renderable_surface_buffer(
                existing,
                pending,
                buffer_size,
                width,
                height,
                placement,
                generation,
                damage,
            )?;
        } else {
            let surface =
                pending.to_renderable_surface(surface_id, placement, generation, damage)?;
            toplevel.window.push_minimized_surface(surface);
        }
        Ok(())
    }

    fn commit_surface_damage_only(
        &mut self,
        surface_id: u32,
        damage: RenderableSurfaceDamage,
        surface_size: Option<BufferSize>,
        buffer_scale: u32,
    ) -> bool {
        let Some(current) = self.current_surface_buffers.get(&surface_id).cloned() else {
            return false;
        };
        let Ok(buffer_width) = current.data.width() else {
            return false;
        };
        let Ok(buffer_height) = current.data.height() else {
            return false;
        };
        let Some(buffer_size) = BufferSize::new(buffer_width, buffer_height) else {
            return false;
        };
        let generation = self.next_render_generation_value();
        let Some(existing) = self
            .renderable_surfaces
            .iter_mut()
            .find(|surface| surface.surface_id == surface_id)
        else {
            return false;
        };

        let damage = if existing.buffer_size() == buffer_size {
            damage.normalized_for_surface(buffer_width, buffer_height)
        } else {
            RenderableSurfaceDamage::Full
        };
        if current.data.is_shm()
            && existing.buffer_size() == buffer_size
            && let Some(pixels) = existing.shm_pixels_mut()
            && current
                .data
                .read_pixels_into_with_damage(pixels, &damage)
                .is_err()
        {
            return false;
        }

        let requested_surface_size =
            match current.surface_size_for_state(surface_size, buffer_scale) {
                Ok(surface_size) => surface_size,
                Err(_) => buffer_size,
            };
        let resize_pending = self
            .resize_configure_flows
            .get(&surface_id)
            .is_some_and(ResizeConfigureFlow::has_in_flight);
        let surface_size = damage_only_rendered_surface_size(
            BufferSize {
                width: existing.width,
                height: existing.height,
            },
            requested_surface_size,
            resize_pending,
        );
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: damage-only commit surface {surface_id} buffer={}x{} requested_surface={}x{} applied_surface={}x{} shm={} dmabuf={} pending_resize={:?}",
                buffer_width,
                buffer_height,
                requested_surface_size.width,
                requested_surface_size.height,
                surface_size.width,
                surface_size.height,
                current.data.is_shm(),
                current.data.is_dmabuf(),
                self.resize_configure_flows
                    .get(&surface_id)
                    .and_then(ResizeConfigureFlow::in_flight_serial),
            );
        }
        existing.x = current.x;
        existing.y = current.y;
        existing.width = surface_size.width;
        existing.height = surface_size.height;
        existing.generation = generation;
        existing.damage = damage;
        self.set_render_generation(generation, RenderGenerationCause::SurfaceDamage);
        true
    }

    fn commit_surface_request(
        &mut self,
        surface_id: u32,
        mut pending: PendingSurfaceBuffer,
        damage: RenderableSurfaceDamage,
        frame_callbacks: Vec<wl_callback::WlCallback>,
        explicit_sync: Option<Arc<SyncobjSurfaceState>>,
    ) {
        if !self.is_cursor_surface(surface_id) {
            self.configure_xdg_surface_if_needed(surface_id);
        }
        let Some(sync_state) = explicit_sync else {
            let mut callbacks = self.cancel_pending_acquire_commits_for_surface(
                surface_id,
                AcquireWatchCancelReason::Superseded,
            );
            callbacks.extend(frame_callbacks);
            pending.resize_commit = self
                .capture_acked_resize_for_surface_commit(surface_id)
                .map(|snapshot| {
                    self.snapshot_resize_commit_for_buffer(surface_id, snapshot, &pending)
                })
                .map(Box::new);
            self.commit_surface_buffer_by_role(surface_id, pending, damage, callbacks);
            return;
        };

        if !pending.data.is_dmabuf() {
            sync_state.post_error(
                SYNCOBJ_SURFACE_ERROR_UNSUPPORTED_BUFFER,
                "explicit sync is only supported for linux-dmabuf buffers",
            );
            return;
        }

        let (acquire, release) = sync_state.take_points();
        let Some(acquire) = acquire else {
            sync_state.post_error(
                SYNCOBJ_SURFACE_ERROR_NO_ACQUIRE_POINT,
                "dmabuf commit is missing an acquire timeline point",
            );
            return;
        };
        let Some(release) = release else {
            sync_state.post_error(
                SYNCOBJ_SURFACE_ERROR_NO_RELEASE_POINT,
                "dmabuf commit is missing a release timeline point",
            );
            return;
        };

        if acquire.timeline.same_timeline(&release.timeline) && acquire.point >= release.point {
            sync_state.post_error(
                SYNCOBJ_SURFACE_ERROR_CONFLICTING_POINTS,
                "acquire timeline point must be lower than release point on the same timeline",
            );
            return;
        }

        pending.explicit_release = Some(release);
        let acquire_ready = acquire.is_signaled();
        if acquire_ready {
            let mut callbacks = self.cancel_pending_acquire_commits_for_surface(
                surface_id,
                AcquireWatchCancelReason::Superseded,
            );
            callbacks.extend(frame_callbacks);
            pending.resize_commit = self
                .capture_acked_resize_for_surface_commit(surface_id)
                .map(|snapshot| {
                    self.snapshot_resize_commit_for_buffer(surface_id, snapshot, &pending)
                })
                .map(Box::new);
            self.commit_surface_buffer_by_role(surface_id, pending, damage, callbacks);
            return;
        }

        let Some(commit_id) = self.acquire_commit_ids.allocate() else {
            sync_state.post_error(
                SYNCOBJ_SURFACE_ERROR_NO_ACQUIRE_POINT,
                "explicit sync commit identity space exhausted",
            );
            return;
        };
        let mut callbacks = self.retain_oldest_pending_acquire_for_surface(surface_id);
        callbacks.extend(frame_callbacks);
        pending.resize_commit = self
            .capture_acked_resize_for_surface_commit(surface_id)
            .map(|snapshot| self.snapshot_resize_commit_for_buffer(surface_id, snapshot, &pending))
            .map(Box::new);
        let buffer_id = pending.resource.id().protocol_id();
        let received_at = Instant::now();
        self.pending_explicit_sync_commits
            .push(PendingExplicitSyncCommit {
                commit_id,
                surface_id,
                pending,
                damage,
                frame_callbacks: callbacks,
                acquire: acquire.clone(),
                acquire_state: PendingAcquireState::RegistrationPending,
            });
        self.resize_flow_metrics.commits_delayed_by_explicit_sync = self
            .resize_flow_metrics
            .commits_delayed_by_explicit_sync
            .saturating_add(1);
        self.resize_flow_metrics.max_pending_explicit_sync_commits = self
            .resize_flow_metrics
            .max_pending_explicit_sync_commits
            .max(self.pending_explicit_sync_commits.len());
        if compositor_debug_surface_logging_enabled() {
            let pending = self
                .pending_explicit_sync_commits
                .last()
                .expect("explicit-sync commit was just queued");
            eprintln!(
                "oblivion-one compositor: resize_flow surface={surface_id} decision=captured commit_generation={} commit_has_buffer=true explicit_sync=waiting acked_serial={:?} pending_explicit_sync={}",
                pending
                    .pending
                    .resize_commit
                    .as_deref()
                    .map_or(0, |snapshot| snapshot.commit_sequence),
                pending
                    .pending
                    .resize_commit
                    .as_deref()
                    .map(|snapshot| snapshot.serial),
                self.pending_explicit_sync_commits.len(),
            );
        }
        if self.external_acquire_readiness {
            self.pending_acquire_watch_changes
                .push(AcquireWatchChange::Register(AcquireWatchRequest {
                    commit_id,
                    surface_id,
                    buffer_id,
                    acquire,
                    received_at,
                }));
        }
    }

    fn commit_surface_without_buffer(
        &mut self,
        surface_id: u32,
        data: &SurfaceData,
        damage: Option<RenderableSurfaceDamage>,
        explicit_sync: Option<Arc<SyncobjSurfaceState>>,
        _window_geometry_changed: bool,
    ) {
        if let Some(sync_state) = explicit_sync {
            let (acquire, release) = sync_state.take_points();
            if acquire.is_some() || release.is_some() {
                sync_state.post_error(
                    SYNCOBJ_SURFACE_ERROR_NO_BUFFER,
                    "explicit sync points were set without an attached buffer",
                );
                return;
            }
        }

        if self.is_cursor_surface(surface_id) {
            let surface_size = data.commit_pending_viewport();
            let buffer_scale = data.commit_pending_buffer_scale();
            if let Some(damage) = damage {
                self.commit_cursor_surface_damage_only(
                    surface_id,
                    damage,
                    surface_size,
                    buffer_scale,
                );
            }
            let callbacks = data.take_frame_callbacks();
            if self
                .client_cursor_render_state()
                .is_some_and(|cursor| cursor.surface.surface_id == surface_id)
            {
                self.pending_frame_callbacks.extend(callbacks);
            } else {
                self.complete_frame_callbacks(callbacks);
            }
            return;
        }

        self.configure_xdg_surface_if_needed(surface_id);
        let mut resize_commit = self.capture_acked_resize_for_surface_commit(surface_id);
        let surface_size = data.commit_pending_viewport();
        let buffer_scale = data.commit_pending_buffer_scale();
        if let Some(snapshot) = resize_commit.as_mut() {
            let committed_size = self
                .xdg_window_geometry_size(surface_id)
                .or_else(|| surface_size.map(|size| (size.width, size.height)))
                .or_else(|| {
                    self.renderable_surfaces
                        .iter()
                        .find(|surface| surface.surface_id == surface_id)
                        .map(|surface| (surface.width, surface.height))
                });
            if let Some((width, height)) = committed_size {
                *snapshot = snapshot.with_committed_size(width, height);
            }
        }
        let viewport_size_changed = surface_size.is_some_and(|surface_size| {
            self.renderable_surfaces
                .iter()
                .find(|surface| surface.surface_id == surface_id)
                .is_some_and(|surface| {
                    surface.width != surface_size.width || surface.height != surface_size.height
                })
        });
        if let Some(damage) =
            damage.or(viewport_size_changed.then_some(RenderableSurfaceDamage::Full))
        {
            self.commit_surface_damage_only(surface_id, damage, surface_size, buffer_scale);
        }
        if let Some(resize_commit) = resize_commit {
            self.complete_pending_resize_from_current_geometry(surface_id, resize_commit);
        }
        self.complete_frame_callbacks_now(data);
    }

    fn complete_pending_resize_from_current_geometry(
        &mut self,
        surface_id: u32,
        resize: ResizeCommitSnapshot,
    ) -> bool {
        let committed_size = resize
            .committed_size
            .map(|(width, height)| BufferSize { width, height })
            .or_else(|| {
                self.renderable_surfaces
                    .iter()
                    .find(|surface| surface.surface_id == surface_id)
                    .map(|surface| BufferSize {
                        width: surface.width,
                        height: surface.height,
                    })
            });
        let Some(committed_size) = committed_size else {
            return false;
        };
        let placement =
            resize.placement_for_committed_size(committed_size.width, committed_size.height);
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: resize commit surface={surface_id} decision=accepted reason=geometry-only serial={} requested={}x{} actual={}x{} placement={},{}",
                resize.serial,
                resize.width,
                resize.height,
                committed_size.width,
                committed_size.height,
                placement.local_x,
                placement.local_y,
            );
        }
        if let Some(surface) = self
            .renderable_surfaces
            .iter_mut()
            .find(|surface| surface.surface_id == surface_id)
        {
            surface.placement = placement;
            surface.resize_preview = None;
            surface.damage = RenderableSurfaceDamage::Full;
        }
        self.store_surface_placement(surface_id, placement);
        self.advance_render_generation(RenderGenerationCause::WindowResize);
        self.complete_applied_resize_transaction(surface_id, resize);
        true
    }

    fn unmap_surface_content(&mut self, surface_id: u32) -> bool {
        let renderable_ids = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        let mut removed_surface_ids = renderable_ids
            .into_iter()
            .filter(|candidate_id| self.surface_is_descendant_of(*candidate_id, surface_id))
            .collect::<Vec<_>>();
        removed_surface_ids.sort_unstable();
        removed_surface_ids.dedup();
        if removed_surface_ids.is_empty() {
            return false;
        }

        for removed_surface_id in &removed_surface_ids {
            self.current_surface_buffers.remove(removed_surface_id);
            if let Some(buffer) = self.active_dmabuf_buffers.remove(removed_surface_id) {
                self.queue_dmabuf_buffer_release(buffer);
            }
        }
        self.clear_resize_state_for_surfaces(&removed_surface_ids);
        self.renderable_surfaces
            .retain(|surface| !removed_surface_ids.contains(&surface.surface_id));
        self.popup_grab_stack
            .retain(|surface_id| !removed_surface_ids.contains(surface_id));
        self.recent_input_serials
            .retain(|input| !removed_surface_ids.contains(&compositor_surface_id(&input.surface)));
        self.clear_pointer_button_state_for_removed_surfaces(
            &removed_surface_ids,
            "surface-destroyed",
        );
        if self
            .pointer_surface
            .as_ref()
            .is_some_and(|surface| removed_surface_ids.contains(&compositor_surface_id(surface)))
        {
            self.clear_pointer_focus();
        }
        if self
            .focused_surface
            .as_ref()
            .is_some_and(|surface| removed_surface_ids.contains(&compositor_surface_id(surface)))
        {
            self.focused_surface = None;
            if self.keyboard_surface.as_ref().is_some_and(|surface| {
                removed_surface_ids.contains(&compositor_surface_id(surface))
            }) {
                self.clear_keyboard_focus();
            }
            let _ = self.focus_topmost_renderable_toplevel();
        }

        self.invalidate_surface_origin_cache();
        self.advance_render_generation(RenderGenerationCause::SurfaceUnmap);
        true
    }

    fn unmap_xdg_role_surfaces(&mut self, surface_id: u32) -> bool {
        let renderable_ids = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        let mut removed_surface_ids = renderable_ids
            .into_iter()
            .filter(|candidate_id| self.surface_is_descendant_of(*candidate_id, surface_id))
            .collect::<Vec<_>>();
        removed_surface_ids.push(surface_id);
        removed_surface_ids.sort_unstable();
        removed_surface_ids.dedup();

        let previous_renderable_count = self.renderable_surfaces.len();
        self.renderable_surfaces
            .retain(|surface| !removed_surface_ids.contains(&surface.surface_id));
        self.popup_grab_stack
            .retain(|surface_id| !removed_surface_ids.contains(surface_id));
        self.recent_input_serials
            .retain(|input| !removed_surface_ids.contains(&compositor_surface_id(&input.surface)));
        self.clear_resize_state_for_surfaces(&removed_surface_ids);
        self.clear_pointer_button_state_for_removed_surfaces(
            &removed_surface_ids,
            "surface-destroyed",
        );
        if self
            .pointer_surface
            .as_ref()
            .is_some_and(|surface| removed_surface_ids.contains(&compositor_surface_id(surface)))
        {
            self.clear_pointer_focus();
        }
        if self
            .focused_surface
            .as_ref()
            .is_some_and(|surface| removed_surface_ids.contains(&compositor_surface_id(surface)))
        {
            self.focused_surface = None;
            if self.keyboard_surface.as_ref().is_some_and(|surface| {
                removed_surface_ids.contains(&compositor_surface_id(surface))
            }) {
                self.clear_keyboard_focus();
            }
            let _ = self.focus_topmost_renderable_toplevel();
        }

        if self.renderable_surfaces.len() == previous_renderable_count {
            return false;
        }

        self.invalidate_surface_origin_cache();
        self.advance_render_generation(RenderGenerationCause::SurfaceUnmap);
        true
    }

    fn clear_resize_state_for_surfaces(&mut self, surface_ids: &[u32]) {
        self.resize_configure_flows
            .retain(|surface_id, _| !surface_ids.contains(surface_id));
        for commit in &mut self.pending_explicit_sync_commits {
            if surface_ids.contains(&commit.surface_id) {
                commit.pending.resize_commit = None;
            }
        }
        self.pending_window_geometry_commits
            .retain(|surface_id| !surface_ids.contains(surface_id));
        self.resize_preview_metadata
            .retain(|surface_id, _| !surface_ids.contains(surface_id));
        let mut restored_placements = Vec::new();
        for surface in &mut self.renderable_surfaces {
            if surface_ids.contains(&surface.surface_id)
                && let Some(preview) = surface.resize_preview.take()
            {
                if preview.anchor_right {
                    let right = surface
                        .placement
                        .local_x
                        .saturating_add(i32::try_from(surface.width).unwrap_or(i32::MAX));
                    surface.placement.local_x = right
                        .saturating_sub(i32::try_from(preview.committed_width).unwrap_or(i32::MAX));
                }
                if preview.anchor_bottom {
                    let bottom = surface
                        .placement
                        .local_y
                        .saturating_add(i32::try_from(surface.height).unwrap_or(i32::MAX));
                    surface.placement.local_y = bottom.saturating_sub(
                        i32::try_from(preview.committed_height).unwrap_or(i32::MAX),
                    );
                }
                surface.width = preview.committed_width;
                surface.height = preview.committed_height;
                restored_placements.push((surface.surface_id, surface.placement));
            }
        }
        for (surface_id, placement) in restored_placements {
            self.surface_placements.insert(surface_id, placement);
        }
        if self
            .window_interaction
            .is_some_and(|interaction| surface_ids.contains(&interaction.root_surface_id))
        {
            self.window_interaction = None;
        }
    }

    fn track_committed_buffer_lifetime(&mut self, surface_id: u32, pending: &PendingSurfaceBuffer) {
        if pending.data.is_shm() {
            if let Some(release) = self.active_dmabuf_buffers.remove(&surface_id) {
                self.queue_dmabuf_buffer_release(release);
            }
            self.queue_buffer_release(pending.resource.clone());
            return;
        }

        let new_release = pending.release_target();
        if let Some(previous) = self
            .active_dmabuf_buffers
            .insert(surface_id, new_release.clone())
            && !previous.same_buffer_resource(&new_release)
        {
            self.queue_dmabuf_buffer_release(previous);
        }
    }

    fn queue_buffer_release(&mut self, buffer: wl_buffer::WlBuffer) {
        self.pending_buffer_releases.push(buffer);
    }

    fn queue_dmabuf_buffer_release(&mut self, release: SurfaceBufferRelease) {
        self.pending_dmabuf_buffer_releases.push(release);
    }

    fn commit_cursor_surface_buffer(
        &mut self,
        surface_id: u32,
        pending: PendingSurfaceBuffer,
        damage: RenderableSurfaceDamage,
        frame_callbacks: Vec<wl_callback::WlCallback>,
    ) {
        self.unmap_surface_content(surface_id);
        let generation = self.next_render_generation_value();
        let Ok(buffer_width) = pending.data.width() else {
            return;
        };
        let Ok(buffer_height) = pending.data.height() else {
            return;
        };
        let damage = damage.normalized_for_surface(buffer_width, buffer_height);
        let Ok(surface) =
            pending.to_renderable_surface(surface_id, SurfacePlacement::root(), generation, damage)
        else {
            return;
        };
        self.track_committed_buffer_lifetime(surface_id, &pending);
        self.current_surface_buffers.insert(surface_id, pending);
        self.client_cursor_surfaces.insert(surface_id, surface);
        self.set_render_generation(generation, RenderGenerationCause::CursorCommit);
        if self
            .active_client_cursor
            .as_ref()
            .is_some_and(|active| active.surface_id == surface_id)
            && self.cursor_visibility.lock_hidden_constraint_id.is_none()
        {
            self.pending_frame_callbacks.extend(frame_callbacks);
        } else {
            self.complete_frame_callbacks(frame_callbacks);
        }
    }

    fn commit_surface_buffer_by_role(
        &mut self,
        surface_id: u32,
        pending: PendingSurfaceBuffer,
        damage: RenderableSurfaceDamage,
        frame_callbacks: Vec<wl_callback::WlCallback>,
    ) {
        if self.is_cursor_surface(surface_id) {
            self.commit_cursor_surface_buffer(surface_id, pending, damage, frame_callbacks);
        } else {
            self.commit_surface_buffer(surface_id, pending, damage);
            self.pending_frame_callbacks.extend(frame_callbacks);
        }
    }

    fn commit_cursor_surface_damage_only(
        &mut self,
        surface_id: u32,
        damage: RenderableSurfaceDamage,
        surface_size: Option<BufferSize>,
        buffer_scale: u32,
    ) -> bool {
        let Some(current) = self.current_surface_buffers.get(&surface_id).cloned() else {
            return false;
        };
        let Ok(buffer_width) = current.data.width() else {
            return false;
        };
        let Ok(buffer_height) = current.data.height() else {
            return false;
        };
        let Some(buffer_size) = BufferSize::new(buffer_width, buffer_height) else {
            return false;
        };
        let generation = self.next_render_generation_value();
        let Some(existing) = self.client_cursor_surfaces.get_mut(&surface_id) else {
            return false;
        };
        let damage = if existing.buffer_size() == buffer_size {
            damage.normalized_for_surface(buffer_width, buffer_height)
        } else {
            RenderableSurfaceDamage::Full
        };
        if current.data.is_shm()
            && existing.buffer_size() == buffer_size
            && let Some(pixels) = existing.shm_pixels_mut()
            && current
                .data
                .read_pixels_into_with_damage(pixels, &damage)
                .is_err()
        {
            return false;
        }
        if let Ok(size) = current.surface_size_for_state(surface_size, buffer_scale) {
            existing.width = size.width;
            existing.height = size.height;
        }
        existing.x = current.x;
        existing.y = current.y;
        existing.generation = generation;
        existing.damage = damage;
        self.set_render_generation(generation, RenderGenerationCause::CursorCommit);
        true
    }

    fn commit_cursor_surface_removal_request(
        &mut self,
        surface_id: u32,
        data: &SurfaceData,
        explicit_sync: Option<Arc<SyncobjSurfaceState>>,
    ) {
        if let Some(sync_state) = explicit_sync {
            let (acquire, release) = sync_state.take_points();
            if acquire.is_some() || release.is_some() {
                sync_state.post_error(
                    SYNCOBJ_SURFACE_ERROR_NO_BUFFER,
                    "explicit sync points were set without an attached buffer",
                );
                return;
            }
        }
        let was_visible = self
            .client_cursor_render_state()
            .is_some_and(|cursor| cursor.surface.surface_id == surface_id);
        let removed = self.client_cursor_surfaces.remove(&surface_id).is_some();
        self.current_surface_buffers.remove(&surface_id);
        if let Some(release) = self.active_dmabuf_buffers.remove(&surface_id) {
            self.queue_dmabuf_buffer_release(release);
        }
        if removed {
            self.advance_render_generation(RenderGenerationCause::CursorCommit);
            pointer_debug_log(format!(
                "cursor surface buffer removed surface={}",
                surface_id
            ));
        }
        let callbacks = data.take_frame_callbacks();
        if was_visible && removed {
            self.pending_frame_callbacks.extend(callbacks);
        } else {
            self.complete_frame_callbacks(callbacks);
        }
    }

    fn surface_placement(&self, surface_id: u32) -> SurfacePlacement {
        self.surface_placements
            .get(&surface_id)
            .copied()
            .unwrap_or_default()
    }

    fn store_surface_placement(&mut self, surface_id: u32, placement: SurfacePlacement) {
        self.invalidate_surface_origin_cache();
        if placement == SurfacePlacement::root() {
            self.surface_placements.remove(&surface_id);
        } else {
            self.surface_placements.insert(surface_id, placement);
        }
    }

    fn register_subsurface_relationship(&mut self, surface_id: u32, parent_id: u32) {
        self.committed_subsurface_stacks
            .entry(parent_id)
            .or_insert_with(|| vec![parent_id])
            .retain(|id| *id == parent_id || *id != surface_id);
        self.committed_subsurface_stacks
            .entry(parent_id)
            .or_insert_with(|| vec![parent_id])
            .push(surface_id);
        self.pending_subsurface_stacks.remove(&parent_id);
        self.reorder_renderable_surfaces_by_committed_stack();
    }

    fn pending_stack_for_parent(&mut self, parent_id: u32) -> &mut Vec<u32> {
        self.pending_subsurface_stacks
            .entry(parent_id)
            .or_insert_with(|| {
                self.committed_subsurface_stacks
                    .get(&parent_id)
                    .cloned()
                    .unwrap_or_else(|| vec![parent_id])
            })
    }

    fn restack_subsurface(
        &mut self,
        surface_id: u32,
        parent_id: u32,
        reference_id: u32,
        above: bool,
    ) -> bool {
        if reference_id == surface_id {
            return false;
        }
        let valid_reference = reference_id == parent_id
            || self
                .surface_placements
                .get(&reference_id)
                .is_some_and(|placement| placement.parent_surface_id == Some(parent_id));
        if !valid_reference {
            return false;
        }

        let stack = self.pending_stack_for_parent(parent_id);
        stack.retain(|id| *id == parent_id || *id != surface_id);
        if !stack.contains(&parent_id) {
            stack.insert(0, parent_id);
        }
        let Some(reference_index) = stack.iter().position(|id| *id == reference_id) else {
            return false;
        };
        let insert_index = if above {
            reference_index + 1
        } else {
            reference_index
        };
        stack.insert(insert_index.min(stack.len()), surface_id);
        true
    }

    fn apply_pending_subsurface_stack_for_parent(&mut self, parent_id: u32) -> bool {
        let Some(mut stack) = self.pending_subsurface_stacks.remove(&parent_id) else {
            return false;
        };
        stack.retain(|id| {
            *id == parent_id
                || self
                    .surface_placements
                    .get(id)
                    .is_some_and(|placement| placement.parent_surface_id == Some(parent_id))
        });
        if !stack.contains(&parent_id) {
            stack.insert(0, parent_id);
        }
        stack.dedup();
        let changed = self
            .committed_subsurface_stacks
            .get(&parent_id)
            .is_none_or(|current| *current != stack);
        self.committed_subsurface_stacks.insert(parent_id, stack);
        if changed {
            self.reorder_renderable_surfaces_by_committed_stack();
            self.refresh_pointer_focus_at_last_position();
        }
        changed
    }

    fn cleanup_subsurface_stack_state_for_surface(&mut self, surface_id: u32) {
        self.committed_subsurface_stacks.remove(&surface_id);
        self.pending_subsurface_stacks.remove(&surface_id);
        for stack in self.committed_subsurface_stacks.values_mut() {
            stack.retain(|id| *id != surface_id);
            stack.dedup();
        }
        for stack in self.pending_subsurface_stacks.values_mut() {
            stack.retain(|id| *id != surface_id);
            stack.dedup();
        }
        self.committed_subsurface_stacks.retain(|parent_id, stack| {
            self.surface_resources.contains_key(parent_id) && stack.iter().any(|id| id != parent_id)
        });
        self.pending_subsurface_stacks.retain(|parent_id, stack| {
            self.surface_resources.contains_key(parent_id) && stack.iter().any(|id| id != parent_id)
        });
        self.reorder_renderable_surfaces_by_committed_stack();
    }

    fn reorder_renderable_surfaces_by_committed_stack(&mut self) -> bool {
        if self.renderable_surfaces.len() <= 1 {
            return false;
        }

        let original_order = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        let mut by_id = self
            .renderable_surfaces
            .drain(..)
            .map(|surface| (surface.surface_id, surface))
            .collect::<HashMap<_, _>>();
        let visible_ids = by_id.keys().copied().collect::<HashSet<_>>();
        let mut ordered_ids = Vec::new();
        let root_ids = original_order
            .iter()
            .copied()
            .filter(|surface_id| {
                self.surface_placements
                    .get(surface_id)
                    .and_then(|placement| placement.parent_surface_id)
                    .is_none_or(|parent_id| !visible_ids.contains(&parent_id))
            })
            .collect::<Vec<_>>();

        for root_id in root_ids {
            self.append_surface_tree_order(root_id, &visible_ids, &mut ordered_ids);
        }
        for surface_id in &original_order {
            if visible_ids.contains(surface_id) && !ordered_ids.contains(surface_id) {
                self.append_surface_tree_order(*surface_id, &visible_ids, &mut ordered_ids);
            }
        }

        self.renderable_surfaces = ordered_ids
            .into_iter()
            .filter_map(|surface_id| by_id.remove(&surface_id))
            .collect();
        let changed = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .ne(original_order);
        if changed {
            self.invalidate_surface_origin_cache();
        }
        changed
    }

    fn append_surface_tree_order(
        &self,
        surface_id: u32,
        visible_ids: &HashSet<u32>,
        ordered_ids: &mut Vec<u32>,
    ) {
        if !visible_ids.contains(&surface_id) || ordered_ids.contains(&surface_id) {
            return;
        }

        if let Some(stack) = self.committed_subsurface_stacks.get(&surface_id) {
            for stacked_id in stack {
                if *stacked_id == surface_id {
                    ordered_ids.push(surface_id);
                } else {
                    self.append_surface_tree_order(*stacked_id, visible_ids, ordered_ids);
                }
            }
        } else {
            ordered_ids.push(surface_id);
        }

        let children = self
            .surface_placements
            .iter()
            .filter_map(|(child_id, placement)| {
                (placement.parent_surface_id == Some(surface_id)
                    && visible_ids.contains(child_id)
                    && !ordered_ids.contains(child_id))
                .then_some(*child_id)
            })
            .collect::<Vec<_>>();
        for child_id in children {
            self.append_surface_tree_order(child_id, visible_ids, ordered_ids);
        }
    }

    fn set_surface_placement(&mut self, surface_id: u32, placement: SurfacePlacement) -> bool {
        self.set_surface_placement_with_cause(
            surface_id,
            placement,
            RenderGenerationCause::SurfacePlacement,
        )
    }

    fn set_surface_placement_with_cause(
        &mut self,
        surface_id: u32,
        placement: SurfacePlacement,
        cause: RenderGenerationCause,
    ) -> bool {
        if self.surface_placement(surface_id) == placement {
            return false;
        }

        self.store_surface_placement(surface_id, placement);

        if let Some(surface) = self
            .renderable_surfaces
            .iter_mut()
            .find(|surface| surface.surface_id == surface_id)
        {
            surface.placement = placement;
            self.advance_render_generation(cause);
            return true;
        }

        false
    }

    fn refresh_surface_origin_cache(&mut self) {
        if self.surface_origin_cache_generation != Some(self.render_generation)
            || self.surface_origin_cache.len() != self.renderable_surfaces.len()
        {
            self.surface_origin_cache = render::surface_origins(&self.renderable_surfaces);
            self.surface_origin_cache_generation = Some(self.render_generation);
        }
    }

    fn invalidate_surface_origin_cache(&mut self) {
        self.surface_origin_cache_generation = None;
    }

    fn raise_renderable_surface_tree(&mut self, surface_id: u32) -> bool {
        let tree_ids = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .filter(|candidate_id| self.surface_is_descendant_of(*candidate_id, surface_id))
            .collect::<HashSet<_>>();
        if tree_ids.is_empty() {
            return false;
        }

        let original_order = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        let mut tree = Vec::new();
        let mut lower = Vec::with_capacity(self.renderable_surfaces.len());
        for surface in self.renderable_surfaces.drain(..) {
            if tree_ids.contains(&surface.surface_id) {
                tree.push(surface);
            } else {
                lower.push(surface);
            }
        }
        lower.extend(tree);
        let changed = lower
            .iter()
            .map(|surface| surface.surface_id)
            .ne(original_order);
        self.renderable_surfaces = lower;
        if changed {
            self.invalidate_surface_origin_cache();
        }
        changed
    }

    fn register_toplevel_surface(
        &mut self,
        surface: wl_surface::WlSurface,
        xdg_surface: xdg_surface::XdgSurface,
        toplevel: xdg_toplevel::XdgToplevel,
    ) {
        let surface_id = compositor_surface_id(&surface);
        if self.is_cursor_surface(surface_id) {
            pointer_debug_log(format!(
                "cursor surface role isolation surface={} rejected=xdg-toplevel",
                surface_id
            ));
            return;
        }
        self.configured_xdg_surfaces.remove(&surface_id);
        self.clear_resize_state_for_surfaces(&[surface_id]);
        self.toplevel_surfaces.insert(
            surface_id,
            ToplevelSurface {
                app_id: None,
                xdg_surface,
                toplevel,
                window: WindowState::default(),
                constraints: Default::default(),
            },
        );
        self.set_surface_placement(surface_id, SurfacePlacement::root());
        self.focus_surface(surface);
    }

    fn register_popup_surface(
        &mut self,
        surface: wl_surface::WlSurface,
        parent: Option<wl_surface::WlSurface>,
        xdg_surface: xdg_surface::XdgSurface,
        popup: xdg_popup::XdgPopup,
        positioner: XdgPositionerState,
    ) {
        let surface_id = compositor_surface_id(&surface);
        if self.is_cursor_surface(surface_id) {
            pointer_debug_log(format!(
                "cursor surface role isolation surface={} rejected=xdg-popup",
                surface_id
            ));
            return;
        }
        self.configured_xdg_surfaces.remove(&surface_id);
        self.clear_resize_state_for_surfaces(&[surface_id]);
        self.popup_surfaces.insert(
            surface_id,
            PopupSurface {
                parent_surface_id: parent.as_ref().map(compositor_surface_id),
                xdg_surface,
                popup,
                positioner,
            },
        );
        self.note_xdg_popup_created();
    }

    fn unregister_toplevel_surface(&mut self, surface_id: u32) {
        self.unmap_xdg_role_surfaces(surface_id);
        self.toplevel_surfaces.remove(&surface_id);
        self.surface_placements.remove(&surface_id);
        self.configured_xdg_surfaces.remove(&surface_id);
        self.xdg_configure_serials.remove(&surface_id);
        self.clear_resize_state_for_surfaces(&[surface_id]);
    }

    fn unregister_xdg_surface_role(&mut self, surface_id: u32) {
        let child_popup_ids = self
            .popup_surfaces
            .iter()
            .filter_map(|(child_surface_id, popup)| {
                (popup.parent_surface_id == Some(surface_id)).then_some(*child_surface_id)
            })
            .collect::<Vec<_>>();
        for child_surface_id in child_popup_ids {
            self.unregister_popup_surface(child_surface_id);
        }

        self.unregister_toplevel_surface(surface_id);
        self.unregister_popup_surface(surface_id);
        self.surface_window_geometries.remove(&surface_id);
        self.configured_xdg_surfaces.remove(&surface_id);
        self.surface_placements.remove(&surface_id);
        self.popup_grab_stack.retain(|id| *id != surface_id);
        self.recent_input_serials
            .retain(|input| compositor_surface_id(&input.surface) != surface_id);
        self.clear_resize_state_for_surfaces(&[surface_id]);
    }

    fn grab_popup_surface(
        &mut self,
        surface: &wl_surface::WlSurface,
        seat: &wl_seat::WlSeat,
        serial: u32,
    ) -> bool {
        let surface_id = compositor_surface_id(surface);
        if !self.popup_surfaces.contains_key(&surface_id)
            || !resource_belongs_to_surface_client(seat, surface)
            || !self.has_recent_input_serial_for_surface(serial, surface)
        {
            self.dismiss_popup_surface(surface_id);
            return false;
        }

        self.popup_grab_stack.retain(|id| *id != surface_id);
        self.popup_grab_stack.push(surface_id);
        self.focus_surface(surface.clone());
        true
    }

    fn unregister_popup_surface(&mut self, surface_id: u32) {
        let parent_surface_id = self
            .popup_surfaces
            .get(&surface_id)
            .and_then(|popup| popup.parent_surface_id);
        self.unmap_xdg_role_surfaces(surface_id);
        if self
            .pointer_surface
            .as_ref()
            .is_some_and(|surface| compositor_surface_id(surface) == surface_id)
        {
            self.clear_pointer_focus();
        }
        self.popup_grab_stack.retain(|id| *id != surface_id);
        self.recent_input_serials
            .retain(|input| compositor_surface_id(&input.surface) != surface_id);
        self.popup_surfaces.remove(&surface_id);
        self.surface_placements.remove(&surface_id);
        self.configured_xdg_surfaces.remove(&surface_id);
        self.surface_window_geometries.remove(&surface_id);
        self.clear_resize_state_for_surfaces(&[surface_id]);
        if self
            .focused_surface
            .as_ref()
            .is_some_and(|surface| compositor_surface_id(surface) == surface_id)
        {
            if let Some(parent_surface) =
                parent_surface_id.and_then(|parent_id| self.surface_resource_by_id(parent_id))
            {
                self.focus_surface(parent_surface);
            } else {
                self.focused_surface = None;
                if self
                    .keyboard_surface
                    .as_ref()
                    .is_some_and(|surface| compositor_surface_id(surface) == surface_id)
                {
                    self.clear_keyboard_focus();
                }
                let _ = self.focus_topmost_renderable_toplevel();
            }
        }
    }

    fn dismiss_popup_surface(&mut self, surface_id: u32) -> bool {
        let child_popup_ids = self
            .popup_surfaces
            .iter()
            .filter_map(|(child_surface_id, popup)| {
                (popup.parent_surface_id == Some(surface_id)).then_some(*child_surface_id)
            })
            .collect::<Vec<_>>();
        for child_surface_id in child_popup_ids {
            self.dismiss_popup_surface(child_surface_id);
        }

        let Some(popup_surface) = self.popup_surfaces.get(&surface_id).cloned() else {
            return false;
        };
        let _ = popup_surface.popup.send_event(xdg_popup::Event::PopupDone);
        self.unregister_popup_surface(surface_id);
        true
    }

    fn popup_grab_to_dismiss_for_pointer_target(
        &self,
        target: Option<&PointerTarget>,
    ) -> Option<u32> {
        let popup_surface_id = self.topmost_popup_grab_surface_id()?;
        if let Some(target) = target {
            let target_surface_id = compositor_surface_id(&target.surface);
            if self.surface_is_descendant_of(target_surface_id, popup_surface_id) {
                return None;
            }
        }

        Some(popup_surface_id)
    }

    fn pointer_target_allowed_by_popup_grab(&self, target: &PointerTarget) -> bool {
        let Some(popup_surface_id) = self.topmost_popup_grab_surface_id() else {
            return true;
        };
        let target_surface_id = compositor_surface_id(&target.surface);
        self.surface_is_descendant_of(target_surface_id, popup_surface_id)
    }

    fn topmost_popup_grab_surface_id(&self) -> Option<u32> {
        self.popup_grab_stack
            .iter()
            .rev()
            .copied()
            .find(|surface_id| self.popup_surfaces.contains_key(surface_id))
    }

    fn surface_is_descendant_of(&self, surface_id: u32, ancestor_surface_id: u32) -> bool {
        let mut current = surface_id;
        for _ in 0..self.surface_placements.len().saturating_add(1) {
            if current == ancestor_surface_id {
                return true;
            }
            let Some(parent_surface_id) = self
                .surface_placements
                .get(&current)
                .copied()
                .and_then(|placement| placement.parent_surface_id)
            else {
                return false;
            };
            if parent_surface_id == current {
                return false;
            }
            current = parent_surface_id;
        }

        false
    }

    fn configure_popup_surface(
        &mut self,
        surface_id: u32,
        positioner: XdgPositionerState,
        reposition_token: Option<u32>,
    ) -> bool {
        if let Some(popup_surface) = self.popup_surfaces.get_mut(&surface_id) {
            popup_surface.positioner = positioner;
        }
        let Some(popup_surface) = self.popup_surfaces.get(&surface_id).cloned() else {
            return false;
        };
        let geometry = positioner
            .constrained_geometry(self.popup_constraint_target(&popup_surface, positioner));
        let parent_window_geometry = popup_surface
            .parent_surface_id
            .and_then(|surface_id| self.surface_window_geometries.get(&surface_id).copied());
        let popup_window_geometry = self.surface_window_geometries.get(&surface_id).copied();
        let local_x = parent_window_geometry
            .map(|geometry| geometry.x)
            .unwrap_or_default()
            .saturating_add(geometry.x)
            .saturating_sub(
                popup_window_geometry
                    .map(|geometry| geometry.x)
                    .unwrap_or_default(),
            );
        let local_y = parent_window_geometry
            .map(|geometry| geometry.y)
            .unwrap_or_default()
            .saturating_add(geometry.y)
            .saturating_sub(
                popup_window_geometry
                    .map(|geometry| geometry.y)
                    .unwrap_or_default(),
            );
        let placement = popup_surface
            .parent_surface_id
            .map(|parent_surface_id| {
                SurfacePlacement::subsurface(parent_surface_id, local_x, local_y)
            })
            .unwrap_or_else(|| SurfacePlacement::root_at(local_x, local_y));

        self.store_surface_placement(surface_id, placement);
        if let Some(token) = reposition_token {
            let _ = popup_surface
                .popup
                .send_event(xdg_popup::Event::Repositioned { token });
        }
        if let Err(error) = popup_surface.popup.send_event(xdg_popup::Event::Configure {
            x: geometry.x,
            y: geometry.y,
            width: geometry.width,
            height: geometry.height,
        }) && compositor_debug_surface_logging_enabled()
        {
            eprintln!("oblivion-one compositor: failed to send popup configure: {error:?}");
        }
        let serial = self.next_configure_serial();
        if let Err(error) = popup_surface
            .xdg_surface
            .send_event(xdg_surface::Event::Configure { serial })
            && compositor_debug_surface_logging_enabled()
        {
            eprintln!(
                "oblivion-one compositor: failed to send popup xdg_surface configure serial={serial}: {error:?}"
            );
        }
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: popup surface {surface_id} configured xdg={}x{}+{},{} placement={},{} parent={:?}",
                geometry.width,
                geometry.height,
                geometry.x,
                geometry.y,
                placement.local_x,
                placement.local_y,
                placement.parent_surface_id
            );
        }
        true
    }

    fn configure_xdg_surface_if_needed(&mut self, surface_id: u32) -> bool {
        if self.configured_xdg_surfaces.contains(&surface_id) {
            return false;
        }

        if let Some(toplevel) = self.toplevel_surfaces.get(&surface_id).cloned() {
            if let Err(error) = toplevel
                .toplevel
                .send_event(xdg_toplevel::Event::Configure {
                    width: 0,
                    height: 0,
                    states: Vec::new(),
                })
                && compositor_debug_surface_logging_enabled()
            {
                eprintln!("oblivion-one compositor: failed to send toplevel configure: {error:?}");
            }
            let serial = self.next_configure_serial();
            if let Err(error) = toplevel
                .xdg_surface
                .send_event(xdg_surface::Event::Configure { serial })
                && compositor_debug_surface_logging_enabled()
            {
                eprintln!(
                    "oblivion-one compositor: failed to send toplevel xdg_surface configure serial={serial}: {error:?}"
                );
            }
            self.configured_xdg_surfaces.insert(surface_id);
            return true;
        }

        let Some(positioner) = self
            .popup_surfaces
            .get(&surface_id)
            .map(|popup| popup.positioner)
        else {
            return false;
        };
        if self.configure_popup_surface(surface_id, positioner, None) {
            self.configured_xdg_surfaces.insert(surface_id);
            return true;
        }

        false
    }

    fn popup_constraint_target(
        &self,
        popup_surface: &PopupSurface,
        positioner: XdgPositionerState,
    ) -> PopupRect {
        if let Some((width, height)) = positioner.parent_size {
            return PopupRect::new(0, 0, width, height);
        }

        if let Some(surface_id) = popup_surface.parent_surface_id
            && let Some(geometry) = self.surface_window_geometries.get(&surface_id).copied()
        {
            return PopupRect::new(0, 0, geometry.width, geometry.height);
        }

        if let Some(surface_id) = popup_surface.parent_surface_id
            && let Some(surface) = self
                .renderable_surfaces
                .iter()
                .find(|surface| surface.surface_id == surface_id)
        {
            return PopupRect::new(0, 0, surface.width as i32, surface.height as i32);
        }

        PopupRect::new(
            0,
            0,
            self.output_size.width as i32,
            self.output_size.height as i32,
        )
    }

    fn begin_window_move_at(&mut self, x: f64, y: f64) -> bool {
        self.begin_window_interaction_at(x, y, WindowInteractionKind::Move)
    }

    fn begin_window_resize_at(&mut self, x: f64, y: f64) -> bool {
        let Some(surface_id) = self.surface_id_at(x, y) else {
            self.window_interaction = None;
            return false;
        };
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        let Some((local_x, local_y, width, height)) =
            self.root_window_local_point_at(root_surface_id, x, y)
        else {
            return self.begin_window_interaction_for_root(
                root_surface_id,
                x,
                y,
                WindowInteractionKind::Resize(ResizeEdges::BOTTOM_RIGHT),
            );
        };
        let edges = resize_edges_for_window_point(local_x, local_y, width, height);
        self.begin_window_interaction_for_root(
            root_surface_id,
            x,
            y,
            WindowInteractionKind::Resize(edges),
        )
    }

    fn begin_window_frame_action_at(&mut self, x: f64, y: f64) -> bool {
        let Some(hit) = self.window_frame_hit_at(x, y) else {
            return false;
        };
        self.begin_window_interaction_for_root(hit.root_surface_id, x, y, hit.kind)
    }

    fn begin_window_interaction_at(&mut self, x: f64, y: f64, kind: WindowInteractionKind) -> bool {
        let Some(surface_id) = self.surface_id_at(x, y) else {
            self.window_interaction = None;
            return false;
        };
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        self.begin_window_interaction_for_root(root_surface_id, x, y, kind)
    }

    fn begin_client_window_move(&mut self, surface: &wl_surface::WlSurface, serial: u32) -> bool {
        let root_surface_id = self.root_surface_id_for_surface(compositor_surface_id(surface));
        let Some((x, y)) = self.valid_pointer_press_for_surface(root_surface_id, surface, serial)
        else {
            return false;
        };
        self.begin_window_interaction_for_root(root_surface_id, x, y, WindowInteractionKind::Move)
    }

    fn begin_client_window_resize(
        &mut self,
        surface: &wl_surface::WlSurface,
        serial: u32,
        edges: ResizeEdges,
    ) -> bool {
        let root_surface_id = self.root_surface_id_for_surface(compositor_surface_id(surface));
        let Some((x, y)) = self.valid_pointer_press_for_surface(root_surface_id, surface, serial)
        else {
            return false;
        };
        self.begin_window_interaction_for_root(
            root_surface_id,
            x,
            y,
            WindowInteractionKind::Resize(edges),
        )
    }

    fn begin_window_interaction_for_root(
        &mut self,
        root_surface_id: u32,
        x: f64,
        y: f64,
        kind: WindowInteractionKind,
    ) -> bool {
        let Some(root_surface) = self
            .renderable_surfaces
            .iter()
            .find(|surface| surface.surface_id == root_surface_id)
        else {
            self.window_interaction = None;
            return false;
        };
        let fallback_geometry = WindowGeometry::new(
            self.surface_placement(root_surface_id),
            root_surface.width,
            root_surface.height,
        );
        let start_geometry = self
            .current_root_window_geometry(root_surface_id)
            .unwrap_or(fallback_geometry);
        let start_width = start_geometry.width;
        let start_height = start_geometry.height;
        let start_placement = start_geometry.placement;
        let Some(root_resource) = self.surface_resource_by_id(root_surface_id) else {
            self.window_interaction = None;
            return false;
        };

        self.focus_surface(root_resource);
        self.window_interaction = Some(WindowInteraction {
            root_surface_id,
            kind,
            start_pointer_x: x,
            start_pointer_y: y,
            start_placement,
            start_width,
            start_height,
            drag_committed: false,
        });
        true
    }

    fn valid_pointer_press_for_surface(
        &self,
        root_surface_id: u32,
        surface: &wl_surface::WlSurface,
        serial: u32,
    ) -> Option<(f64, f64)> {
        let press = self.last_pointer_press.as_ref()?;
        let valid_surface = press.root_surface_id == root_surface_id
            || press.surface.id().same_client_as(&surface.id());
        (press.serial == serial && valid_surface).then_some((press.output_x, press.output_y))
    }

    fn window_frame_hit_at(&mut self, x: f64, y: f64) -> Option<WindowFrameHit> {
        if let Some(hit) = self.root_surface_hit_at(x, y) {
            let kind = window_frame_action_for_local_point(
                hit.local_x,
                hit.local_y,
                hit.width,
                hit.height,
            )?;
            return Some(WindowFrameHit {
                root_surface_id: hit.root_surface_id,
                kind,
            });
        }

        None
    }

    fn update_window_interaction(&mut self, x: f64, y: f64) -> bool {
        let Some(mut interaction) = self.window_interaction else {
            return false;
        };
        let dx = (x - interaction.start_pointer_x).round() as i32;
        let dy = (y - interaction.start_pointer_y).round() as i32;

        match interaction.kind {
            WindowInteractionKind::Move => {
                let placement = SurfacePlacement::root_at(
                    interaction.start_placement.local_x + dx,
                    interaction.start_placement.local_y + dy,
                );
                self.set_surface_placement_with_cause(
                    interaction.root_surface_id,
                    placement,
                    RenderGenerationCause::WindowMove,
                )
            }
            WindowInteractionKind::Resize(edges) => {
                if !interaction.drag_committed && !resize_drag_threshold_reached(edges, dx, dy) {
                    return false;
                }
                interaction.drag_committed = true;
                self.window_interaction = Some(interaction);

                let resize = interactive_resize_geometry(interaction, edges, dx, dy);
                self.queue_resize_root_window_to(
                    interaction.root_surface_id,
                    resize.width,
                    resize.height,
                    SurfacePlacement::root_at(resize.x, resize.y),
                    edges,
                )
            }
        }
    }

    fn end_window_interaction(&mut self) {
        let interaction = self.window_interaction;
        if let Some(interaction) = interaction
            && interaction.drag_committed
            && let WindowInteractionKind::Resize(edges) = interaction.kind
        {
            self.send_resize_end_configure(interaction.root_surface_id, edges);
        }
        self.window_interaction = None;
    }

    fn window_interaction_active(&self) -> bool {
        self.window_interaction.is_some()
    }

    fn resize_focused_window_to(&mut self, width: u32, height: u32) -> bool {
        let Some(surface_id) = self.focused_surface.as_ref().map(compositor_surface_id) else {
            return false;
        };
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        self.resize_root_window_to(root_surface_id, width, height)
    }

    fn minimize_focused_window(&mut self) -> bool {
        let Some(surface_id) = self.focused_root_surface_id() else {
            return false;
        };
        self.minimize_root_window(surface_id)
    }

    fn restore_next_minimized_window(&mut self) -> bool {
        let Some(surface_id) = self
            .toplevel_surfaces
            .iter()
            .find_map(|(surface_id, toplevel)| {
                toplevel.window.is_minimized().then_some(*surface_id)
            })
        else {
            return false;
        };

        self.restore_minimized_root_window(surface_id)
    }

    fn activate_root_window(&mut self, surface_id: u32) -> bool {
        if !self.toplevel_surfaces.contains_key(&surface_id) {
            return false;
        }
        if self
            .toplevel_surfaces
            .get(&surface_id)
            .is_some_and(|toplevel| toplevel.window.is_minimized())
        {
            self.restore_minimized_root_window(surface_id);
        }
        let focused = self
            .surface_resource_by_id(surface_id)
            .map(|surface| {
                self.focus_surface(surface);
                true
            })
            .unwrap_or(false);
        let raised = self.raise_root_window(surface_id);
        focused || raised
    }

    fn toggle_maximize_focused_window(&mut self) -> bool {
        let Some(surface_id) = self.focused_root_surface_id() else {
            return false;
        };
        self.toggle_root_window_mode(surface_id, ToplevelMode::Maximized)
    }

    fn toggle_fullscreen_focused_window(&mut self) -> bool {
        let Some(surface_id) = self.focused_root_surface_id() else {
            return false;
        };
        self.toggle_root_window_mode(surface_id, ToplevelMode::Fullscreen)
    }

    fn minimize_root_window(&mut self, surface_id: u32) -> bool {
        if !self.toplevel_surfaces.contains_key(&surface_id)
            || self
                .toplevel_surfaces
                .get(&surface_id)
                .is_some_and(|toplevel| toplevel.window.is_minimized())
        {
            return false;
        }
        self.clear_resize_state_for_surfaces(&[surface_id]);

        let surface_placements = &self.surface_placements;
        let mut minimized_surfaces = Vec::new();
        let mut visible_surfaces = Vec::with_capacity(self.renderable_surfaces.len());
        for surface in self.renderable_surfaces.drain(..) {
            if root_surface_id_for_surface_in_placements(surface_placements, surface.surface_id)
                == surface_id
            {
                minimized_surfaces.push(surface);
            } else {
                visible_surfaces.push(surface);
            }
        }
        self.renderable_surfaces = visible_surfaces;

        if minimized_surfaces.is_empty() {
            return false;
        }

        if let Some(toplevel) = self.toplevel_surfaces.get_mut(&surface_id) {
            toplevel.window.minimize(minimized_surfaces);
        }
        if self.focused_root_surface_id() == Some(surface_id) {
            self.focused_surface = None;
            self.clear_keyboard_focus();
            if self.pointer_surface.as_ref().is_some_and(|surface| {
                self.root_surface_id_for_surface(compositor_surface_id(surface)) == surface_id
            }) {
                self.clear_pointer_focus();
            }
        }
        self.focus_topmost_renderable_toplevel();
        self.advance_render_generation(RenderGenerationCause::WindowMinimize);
        true
    }

    fn restore_minimized_root_window(&mut self, surface_id: u32) -> bool {
        let Some(minimized_surfaces) = self
            .toplevel_surfaces
            .get_mut(&surface_id)
            .and_then(|toplevel| toplevel.window.restore_minimized())
        else {
            return false;
        };

        self.renderable_surfaces.extend(minimized_surfaces);
        if let Some(surface) = self.surface_resource_by_id(surface_id) {
            self.focus_surface(surface);
        }
        self.advance_render_generation(RenderGenerationCause::WindowRestore);
        true
    }

    fn toggle_root_window_mode(&mut self, surface_id: u32, mode: ToplevelMode) -> bool {
        let Some(current_mode) = self
            .toplevel_surfaces
            .get(&surface_id)
            .map(|toplevel| toplevel.window.mode())
        else {
            return false;
        };

        if current_mode == mode {
            self.restore_floating_root_window(surface_id)
        } else {
            self.set_root_window_mode(surface_id, mode)
        }
    }

    fn set_root_window_mode(&mut self, surface_id: u32, mode: ToplevelMode) -> bool {
        if !self.toplevel_surfaces.contains_key(&surface_id) {
            return false;
        }
        self.clear_resize_state_for_surfaces(&[surface_id]);
        if self
            .toplevel_surfaces
            .get(&surface_id)
            .is_some_and(|toplevel| toplevel.window.is_minimized())
        {
            self.restore_minimized_root_window(surface_id);
        }

        let restore_geometry = self
            .current_root_window_geometry(surface_id)
            .unwrap_or_else(|| WindowGeometry::new(self.surface_placement(surface_id), 0, 0));
        if let Some(toplevel) = self.toplevel_surfaces.get_mut(&surface_id) {
            toplevel.window.capture_restore_geometry(restore_geometry);
            toplevel.window.set_mode(mode);
        }

        let states = mode.xdg_states();
        let configured = self
            .send_configure_root_window_to(
                surface_id,
                self.output_size.width,
                self.output_size.height,
                states,
            )
            .is_some();
        let fullscreen_placement = SurfacePlacement::root_at(
            -render::FIRST_SURFACE_OFFSET.0,
            -render::FIRST_SURFACE_OFFSET.1,
        );
        self.set_surface_placement_with_cause(
            surface_id,
            fullscreen_placement,
            RenderGenerationCause::WindowMode,
        );
        configured
    }

    fn restore_floating_root_window(&mut self, surface_id: u32) -> bool {
        self.clear_resize_state_for_surfaces(&[surface_id]);
        let Some(restore_geometry) = self.toplevel_surfaces.get_mut(&surface_id).map(|toplevel| {
            toplevel.window.set_mode(ToplevelMode::Floating);
            toplevel.window.take_restore_geometry()
        }) else {
            return false;
        };
        let restore_geometry = restore_geometry
            .or_else(|| self.current_root_window_geometry(surface_id))
            .unwrap_or_else(|| WindowGeometry::new(self.surface_placement(surface_id), 0, 0));

        let configured = self
            .send_configure_root_window_to(
                surface_id,
                restore_geometry.width,
                restore_geometry.height,
                &[],
            )
            .is_some();
        self.set_surface_placement_with_cause(
            surface_id,
            restore_geometry.placement,
            RenderGenerationCause::WindowMode,
        );
        configured
    }

    fn focused_root_surface_id(&self) -> Option<u32> {
        self.focused_surface
            .as_ref()
            .map(|surface| self.root_surface_id_for_surface(compositor_surface_id(surface)))
    }

    fn current_root_window_geometry(&self, surface_id: u32) -> Option<WindowGeometry> {
        let surface = self
            .renderable_surfaces
            .iter()
            .find(|surface| surface.surface_id == surface_id)
            .or_else(|| {
                self.toplevel_surfaces
                    .get(&surface_id)?
                    .window
                    .minimized_root_surface(surface_id)
            })?;
        let (width, height) = self
            .xdg_window_geometry_size(surface_id)
            .unwrap_or((surface.width, surface.height));

        Some(WindowGeometry::new(
            self.surface_placement(surface_id),
            width,
            height,
        ))
    }

    fn xdg_window_geometry_size(&self, surface_id: u32) -> Option<(u32, u32)> {
        let geometry = self.surface_window_geometries.get(&surface_id)?;
        Some((
            u32::try_from(geometry.width).ok()?,
            u32::try_from(geometry.height).ok()?,
        ))
    }

    fn focus_topmost_renderable_toplevel(&mut self) -> bool {
        let Some(surface_id) = self.renderable_surfaces.iter().rev().find_map(|surface| {
            let root_surface_id = self.root_surface_id_for_surface(surface.surface_id);
            self.toplevel_surfaces
                .contains_key(&root_surface_id)
                .then_some(root_surface_id)
        }) else {
            return false;
        };
        let Some(surface) = self.surface_resource_by_id(surface_id) else {
            return false;
        };
        self.focus_surface(surface);
        true
    }

    fn raise_root_window(&mut self, surface_id: u32) -> bool {
        let surface_placements = &self.surface_placements;
        let mut raised_surfaces = Vec::new();
        let mut lower_surfaces = Vec::with_capacity(self.renderable_surfaces.len());
        for surface in self.renderable_surfaces.drain(..) {
            if root_surface_id_for_surface_in_placements(surface_placements, surface.surface_id)
                == surface_id
            {
                raised_surfaces.push(surface);
            } else {
                lower_surfaces.push(surface);
            }
        }
        if raised_surfaces.is_empty() {
            self.renderable_surfaces = lower_surfaces;
            return false;
        }
        lower_surfaces.extend(raised_surfaces);
        self.renderable_surfaces = lower_surfaces;
        self.advance_render_generation(RenderGenerationCause::WindowStack);
        true
    }

    fn shell_dock_items(&self) -> Vec<ShellDockItem> {
        let focused_root_surface_id = self.focused_root_surface_id();
        let mut surface_ids = self
            .renderable_surfaces
            .iter()
            .map(|surface| self.root_surface_id_for_surface(surface.surface_id))
            .filter(|surface_id| self.toplevel_surfaces.contains_key(surface_id))
            .collect::<Vec<_>>();
        surface_ids.sort_unstable();
        surface_ids.dedup();
        let mut known_surface_ids = surface_ids.iter().copied().collect::<HashSet<_>>();
        for surface_id in self.toplevel_surfaces.keys().copied() {
            if known_surface_ids.insert(surface_id) {
                surface_ids.push(surface_id);
            }
        }
        surface_ids
            .into_iter()
            .filter_map(|surface_id| {
                let toplevel = self.toplevel_surfaces.get(&surface_id)?;
                let label = toplevel
                    .app_id
                    .clone()
                    .unwrap_or_else(|| format!("app-{surface_id}"));
                Some(ShellDockItem::new(
                    surface_id,
                    label,
                    focused_root_surface_id == Some(surface_id),
                    toplevel.window.is_minimized(),
                ))
            })
            .collect()
    }

    fn resize_root_window_to(&mut self, surface_id: u32, width: u32, height: u32) -> bool {
        self.send_resize_root_window_to(surface_id, width, height)
    }

    fn queue_resize_root_window_to(
        &mut self,
        surface_id: u32,
        width: u32,
        height: u32,
        placement: SurfacePlacement,
        edges: ResizeEdges,
    ) -> bool {
        if !self.toplevel_surfaces.contains_key(&surface_id) {
            return false;
        };
        let geometry = self.clamp_resize_geometry(
            surface_id,
            WindowGeometry::new(placement, width, height),
            edges,
        );
        let width = geometry.width;
        let height = geometry.height;
        let placement = geometry.placement;
        let pending = PendingResizeConfigure {
            surface_id,
            width,
            height,
            placement,
            edges,
            resizing: true,
        };
        self.resize_flow_metrics.configures_requested = self
            .resize_flow_metrics
            .configures_requested
            .saturating_add(1);
        let flow = self.resize_configure_flows.entry(surface_id).or_default();
        let was_blocked = flow.has_in_flight() || flow.latest_desired().is_some();
        let queued = flow.queue(pending);
        if queued && was_blocked {
            self.resize_flow_metrics.geometries_coalesced = self
                .resize_flow_metrics
                .geometries_coalesced
                .saturating_add(1);
            if compositor_debug_surface_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: resize_flow surface={surface_id} decision=coalesced queued_serial=not-sent queued_size={}x{} final_pending=false preview_active=true",
                    pending.width, pending.height,
                );
            }
        }
        self.preview_resize_root_window_to(surface_id, width, height, placement, edges)
    }

    fn clamp_resize_geometry(
        &self,
        surface_id: u32,
        geometry: WindowGeometry,
        edges: ResizeEdges,
    ) -> WindowGeometry {
        let width = self.clamp_toplevel_width(surface_id, geometry.width);
        let height = self.clamp_toplevel_height(surface_id, geometry.height);
        let mut placement = geometry.placement;
        if edges.left && width != geometry.width {
            let requested_right = placement
                .local_x
                .saturating_add(i32::try_from(geometry.width).unwrap_or(i32::MAX));
            placement.local_x =
                requested_right.saturating_sub(i32::try_from(width).unwrap_or(i32::MAX));
        }
        if edges.top && height != geometry.height {
            let requested_bottom = placement
                .local_y
                .saturating_add(i32::try_from(geometry.height).unwrap_or(i32::MAX));
            placement.local_y =
                requested_bottom.saturating_sub(i32::try_from(height).unwrap_or(i32::MAX));
        }

        WindowGeometry::new(placement, width, height)
    }

    fn clamp_toplevel_width(&self, surface_id: u32, width: u32) -> u32 {
        let constraints = self.toplevel_constraints(surface_id);
        let min_width = constraints.min_width.unwrap_or(MIN_WINDOW_WIDTH);
        let mut clamped = width.max(min_width);
        if let Some(max_width) = constraints.max_width {
            clamped = clamped.min(max_width.max(min_width));
        }
        clamped
    }

    fn clamp_toplevel_height(&self, surface_id: u32, height: u32) -> u32 {
        let constraints = self.toplevel_constraints(surface_id);
        let min_height = constraints.min_height.unwrap_or(MIN_WINDOW_HEIGHT);
        let mut clamped = height.max(min_height);
        if let Some(max_height) = constraints.max_height {
            clamped = clamped.min(max_height.max(min_height));
        }
        clamped
    }

    fn toplevel_constraints(&self, surface_id: u32) -> ToplevelSizeConstraints {
        self.toplevel_surfaces
            .get(&surface_id)
            .map(|toplevel| toplevel.constraints)
            .unwrap_or_default()
    }

    fn preview_resize_root_window_to(
        &mut self,
        surface_id: u32,
        width: u32,
        height: u32,
        placement: SurfacePlacement,
        edges: ResizeEdges,
    ) -> bool {
        let preview_was_active = self
            .renderable_surfaces
            .iter()
            .find(|surface| surface.surface_id == surface_id)
            .is_some_and(|surface| surface.resize_preview.is_some());
        let flow_sequence = self
            .resize_configure_flows
            .get(&surface_id)
            .and_then(ResizeConfigureFlow::in_flight_sequence)
            .unwrap_or_else(|| self.next_resize_configure_sequence.saturating_add(1));
        let Some(surface) = self
            .renderable_surfaces
            .iter_mut()
            .find(|surface| surface.surface_id == surface_id)
        else {
            return false;
        };
        if surface.width == width && surface.height == height && surface.placement == placement {
            return false;
        }

        let committed_width = surface
            .resize_preview
            .map(|preview| preview.committed_width)
            .unwrap_or(surface.width);
        let committed_height = surface
            .resize_preview
            .map(|preview| preview.committed_height)
            .unwrap_or(surface.height);
        surface.width = width;
        surface.height = height;
        surface.placement = placement;
        surface.resize_preview = Some(ResizePreview {
            committed_width,
            committed_height,
            anchor_right: edges.left,
            anchor_bottom: edges.top,
        });
        surface.damage = RenderableSurfaceDamage::Full;
        if !preview_was_active {
            self.resize_preview_metadata.insert(
                surface_id,
                ResizePreviewMetadata {
                    flow_sequence,
                    activated_at: Instant::now(),
                },
            );
            self.resize_flow_metrics.preview_activations = self
                .resize_flow_metrics
                .preview_activations
                .saturating_add(1);
        }
        self.store_surface_placement(surface_id, placement);
        self.advance_render_generation(RenderGenerationCause::WindowResize);
        true
    }

    fn flush_pending_resize_configure(&mut self) -> bool {
        let surface_ids = self
            .resize_configure_flows
            .iter()
            .filter_map(|(surface_id, flow)| flow.has_sendable().then_some(*surface_id))
            .collect::<Vec<_>>();
        let mut sent = false;
        for surface_id in surface_ids {
            let desired = self
                .resize_configure_flows
                .get_mut(&surface_id)
                .and_then(ResizeConfigureFlow::take_sendable);
            if let Some(desired) = desired {
                sent |= self.send_resize_configure(desired);
            }
        }
        sent
    }

    fn send_resize_end_configure(&mut self, surface_id: u32, edges: ResizeEdges) -> bool {
        let desired = self
            .resize_configure_flows
            .get(&surface_id)
            .and_then(ResizeConfigureFlow::latest_desired)
            .map(|pending| PendingResizeConfigure {
                resizing: false,
                ..pending
            })
            .or_else(|| {
                self.current_root_window_geometry(surface_id)
                    .map(|geometry| PendingResizeConfigure {
                        surface_id,
                        width: geometry.width,
                        height: geometry.height,
                        placement: geometry.placement,
                        edges,
                        resizing: false,
                    })
            });
        let Some(desired) = desired else {
            return false;
        };
        self.resize_flow_metrics.configures_requested = self
            .resize_flow_metrics
            .configures_requested
            .saturating_add(1);
        self.resize_configure_flows
            .entry(surface_id)
            .or_default()
            .queue_final(desired);
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: resize_flow surface={surface_id} decision=coalesced queued_serial=not-sent queued_size={}x{} final_pending=true preview_active={}",
                desired.width,
                desired.height,
                self.resize_preview_metadata.contains_key(&surface_id),
            );
        }
        self.flush_pending_resize_configure()
    }

    fn pending_resize_configure_is_flushable(&self) -> bool {
        self.resize_configure_flows
            .values()
            .any(ResizeConfigureFlow::has_sendable)
    }

    fn send_resize_configure(&mut self, desired: PendingResizeConfigure) -> bool {
        let surface_id = desired.surface_id;
        let geometry = self.clamp_resize_geometry(
            surface_id,
            WindowGeometry::new(desired.placement, desired.width, desired.height),
            desired.edges,
        );
        let width = geometry.width;
        let height = geometry.height;
        let placement = geometry.placement;
        let resizing_states = [xdg_toplevel::State::Resizing];
        let states = if desired.resizing {
            &resizing_states[..]
        } else {
            &[][..]
        };
        let Some(serial) = self.send_configure_root_window_to(surface_id, width, height, states)
        else {
            return false;
        };
        let resize = PendingResizeConfigure {
            surface_id,
            width: width.max(MIN_WINDOW_WIDTH),
            height: height.max(MIN_WINDOW_HEIGHT),
            placement,
            edges: desired.edges,
            resizing: desired.resizing,
        };
        self.next_resize_configure_sequence = self.next_resize_configure_sequence.saturating_add(1);
        let sequence = self.next_resize_configure_sequence;
        self.resize_configure_flows
            .entry(surface_id)
            .or_default()
            .mark_sent(resize, serial, sequence);
        self.resize_flow_metrics.configures_sent =
            self.resize_flow_metrics.configures_sent.saturating_add(1);
        self.resize_flow_metrics.max_in_flight_configures = self
            .resize_flow_metrics
            .max_in_flight_configures
            .max(usize::from(
                self.resize_configure_flows
                    .get(&surface_id)
                    .is_some_and(ResizeConfigureFlow::has_in_flight),
            ));
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: resize_flow surface={surface_id} decision=sent serial={serial} sequence={sequence} size={}x{} placement={},{} edges={:?} resizing={} in_flight_serial={serial}",
                resize.width,
                resize.height,
                resize.placement.local_x,
                resize.placement.local_y,
                resize.edges,
                resize.resizing,
            );
        }
        true
    }

    fn take_pending_resize_commit_placement(
        &self,
        surface_id: u32,
        pending: &PendingSurfaceBuffer,
    ) -> io::Result<Option<SurfacePlacement>> {
        let Some(resize) = pending.resize_commit.as_deref().copied() else {
            return Ok(None);
        };
        let buffer_width = pending.data.width()?;
        let buffer_height = pending.data.height()?;
        let committed_size = resize
            .committed_size
            .map(|(width, height)| BufferSize { width, height })
            .or(pending.surface_size)
            .unwrap_or(BufferSize {
                width: buffer_width,
                height: buffer_height,
            });
        let placement =
            resize.placement_for_committed_size(committed_size.width, committed_size.height);
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: resize commit surface={surface_id} decision=accepted serial={} requested={}x{} actual={}x{} placement={},{}",
                resize.serial,
                resize.width,
                resize.height,
                committed_size.width,
                committed_size.height,
                placement.local_x,
                placement.local_y,
            );
        }
        Ok(Some(placement))
    }

    fn ack_xdg_surface_configure(&mut self, surface_id: u32, serial: u32) {
        if !self.toplevel_surfaces.contains_key(&surface_id) {
            if compositor_debug_surface_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: resize_flow surface={surface_id} acked_serial={serial} decision=acked reason=matched_other_configure"
                );
            }
            return;
        }
        let resize_decision = self
            .resize_configure_flows
            .get_mut(&surface_id)
            .map_or(ResizeAckDecision::Unknown, |flow| flow.ack(serial));
        let serial_state = self.xdg_configure_serials.entry(surface_id).or_default();
        let matched_other = resize_decision == ResizeAckDecision::Unknown
            && serial == serial_state.latest_sent
            && serial > serial_state.latest_acked;
        let decision = if matched_other {
            "matched_other_configure"
        } else {
            match resize_decision {
                ResizeAckDecision::Matched => "matched_in_flight",
                ResizeAckDecision::Duplicate => "duplicate_serial",
                ResizeAckDecision::Stale => "stale_serial",
                ResizeAckDecision::Unknown if serial <= serial_state.latest_sent => "stale_serial",
                ResizeAckDecision::Unknown => "unknown_serial",
            }
        };
        if matched_other || resize_decision == ResizeAckDecision::Matched {
            serial_state.latest_acked = serial_state.latest_acked.max(serial);
        }
        match resize_decision {
            ResizeAckDecision::Matched => {
                self.resize_flow_metrics.acks_matched =
                    self.resize_flow_metrics.acks_matched.saturating_add(1);
            }
            ResizeAckDecision::Stale | ResizeAckDecision::Duplicate => {
                self.resize_flow_metrics.acks_stale =
                    self.resize_flow_metrics.acks_stale.saturating_add(1);
            }
            ResizeAckDecision::Unknown => {
                if !matched_other && serial > serial_state.latest_sent {
                    self.resize_flow_metrics.acks_unknown =
                        self.resize_flow_metrics.acks_unknown.saturating_add(1);
                } else if !matched_other {
                    self.resize_flow_metrics.acks_stale =
                        self.resize_flow_metrics.acks_stale.saturating_add(1);
                }
            }
        }
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: resize_flow surface={surface_id} acked_serial={serial} decision={} reason={decision}",
                if resize_decision == ResizeAckDecision::Matched || matched_other {
                    "acked"
                } else {
                    "ignored"
                },
            );
        }
    }

    fn capture_acked_resize_for_surface_commit(
        &mut self,
        surface_id: u32,
    ) -> Option<ResizeCommitSnapshot> {
        if !self.toplevel_surfaces.contains_key(&surface_id) {
            return None;
        }
        self.next_surface_commit_sequence = self.next_surface_commit_sequence.saturating_add(1);
        let commit_sequence = self.next_surface_commit_sequence;
        let snapshot = self
            .resize_configure_flows
            .get_mut(&surface_id)
            .and_then(|flow| flow.capture(commit_sequence));
        if let Some(snapshot) = snapshot {
            self.resize_flow_metrics.commits_captured =
                self.resize_flow_metrics.commits_captured.saturating_add(1);
            if compositor_debug_surface_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: resize_flow surface={surface_id} decision=captured acked_serial={} sequence={} commit_generation={} resizing={}",
                    snapshot.serial, snapshot.sequence, snapshot.commit_sequence, snapshot.resizing,
                );
            }
        }
        snapshot
    }

    fn snapshot_resize_commit_for_buffer(
        &self,
        surface_id: u32,
        snapshot: ResizeCommitSnapshot,
        pending: &PendingSurfaceBuffer,
    ) -> ResizeCommitSnapshot {
        let snapshot = snapshot.with_buffer_id(pending.data.buffer_id().get());
        let committed_size = self
            .xdg_window_geometry_size(surface_id)
            .map(|(width, height)| BufferSize { width, height })
            .or(pending.surface_size)
            .or_else(|| {
                Some(BufferSize {
                    width: pending.data.width().ok()?,
                    height: pending.data.height().ok()?,
                })
            });
        committed_size.map_or(snapshot, |size| {
            snapshot.with_committed_size(size.width, size.height)
        })
    }

    fn complete_applied_resize_transaction(
        &mut self,
        surface_id: u32,
        snapshot: ResizeCommitSnapshot,
    ) -> bool {
        let completed = self
            .resize_configure_flows
            .get_mut(&surface_id)
            .is_some_and(|flow| flow.complete_applied(snapshot.sequence));
        if !completed {
            return false;
        }
        self.resize_flow_metrics.preview_completions = self
            .resize_flow_metrics
            .preview_completions
            .saturating_add(1);
        let preview_metadata = self.resize_preview_metadata.remove(&surface_id);
        let preview_sequence = preview_metadata.map(|metadata| metadata.flow_sequence);
        let preview_age = preview_metadata
            .map(|metadata| metadata.activated_at.elapsed())
            .unwrap_or_else(|| snapshot.emitted_at.elapsed());
        let preview_age_ms = u64::try_from(preview_age.as_millis()).unwrap_or(u64::MAX);
        self.resize_flow_metrics.max_preview_age_ms = self
            .resize_flow_metrics
            .max_preview_age_ms
            .max(preview_age_ms);
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: resize_flow surface={surface_id} decision=applied serial={} sequence={} commit_generation={} buffer_id={:?} preview_sequence={preview_sequence:?} preview_active=false preview_age_ms={preview_age_ms}",
                snapshot.serial, snapshot.sequence, snapshot.commit_sequence, snapshot.buffer_id,
            );
        }
        self.flush_pending_resize_configure();
        if self
            .resize_configure_flows
            .get(&surface_id)
            .is_some_and(ResizeConfigureFlow::is_empty)
        {
            self.resize_configure_flows.remove(&surface_id);
        }
        true
    }

    fn send_resize_root_window_to(&mut self, surface_id: u32, width: u32, height: u32) -> bool {
        self.send_configure_root_window_to(surface_id, width, height, &[])
            .is_some()
    }

    fn send_configure_root_window_to(
        &mut self,
        surface_id: u32,
        width: u32,
        height: u32,
        states: &[xdg_toplevel::State],
    ) -> Option<u32> {
        let width = self.clamp_toplevel_width(surface_id, width);
        let height = self.clamp_toplevel_height(surface_id, height);
        let toplevel = self.toplevel_surfaces.get(&surface_id).cloned()?;

        let _ = toplevel
            .toplevel
            .send_event(xdg_toplevel::Event::Configure {
                width: width as i32,
                height: height as i32,
                states: xdg_toplevel_state_bytes(states),
            });
        let serial = self.next_configure_serial();
        let _ = toplevel
            .xdg_surface
            .send_event(xdg_surface::Event::Configure { serial });
        self.xdg_configure_serials
            .entry(surface_id)
            .or_default()
            .latest_sent = serial;
        Some(serial)
    }

    fn surface_id_at(&mut self, x: f64, y: f64) -> Option<u32> {
        self.refresh_surface_origin_cache();
        let origins = &self.surface_origin_cache;
        for (index, renderable) in self.renderable_surfaces.iter().enumerate().rev() {
            let Some(origin) = origins.get(index).copied() else {
                continue;
            };
            let Some((surface_x, surface_y)) =
                render::surface_local_point_at_origin(renderable, origin, x, y)
            else {
                continue;
            };
            if self.surface_accepts_input_at(renderable, surface_x, surface_y) {
                return Some(renderable.surface_id);
            }
        }

        None
    }

    fn root_surface_hit_at(&mut self, x: f64, y: f64) -> Option<RootSurfaceHit> {
        self.refresh_surface_origin_cache();
        let origins = &self.surface_origin_cache;
        for (index, renderable) in self.renderable_surfaces.iter().enumerate().rev() {
            let Some(origin) = origins.get(index).copied() else {
                continue;
            };

            let root_surface_id = self.root_surface_id_for_surface(renderable.surface_id);
            if !self.toplevel_surfaces.contains_key(&root_surface_id) {
                continue;
            }
            let Some(root_index) = self
                .renderable_surfaces
                .iter()
                .position(|surface| surface.surface_id == root_surface_id)
            else {
                continue;
            };
            let Some(root_origin) = origins.get(root_index).copied() else {
                continue;
            };
            let root_surface = &self.renderable_surfaces[root_index];
            let local_x = x - f64::from(root_origin.0);
            let local_y = y - f64::from(root_origin.1);
            if window_frame_action_for_local_point(
                local_x,
                local_y,
                root_surface.width,
                root_surface.height,
            )
            .is_some()
            {
                return Some(RootSurfaceHit {
                    root_surface_id,
                    local_x,
                    local_y,
                    width: root_surface.width,
                    height: root_surface.height,
                });
            }

            if let Some((surface_x, surface_y)) =
                render::surface_local_point_at_origin(renderable, origin, x, y)
                && self.surface_accepts_input_at(renderable, surface_x, surface_y)
            {
                return None;
            }
        }

        None
    }

    fn root_surface_id_for_surface(&self, surface_id: u32) -> u32 {
        root_surface_id_for_surface_in_placements(&self.surface_placements, surface_id)
    }

    fn root_window_local_point_at(
        &mut self,
        root_surface_id: u32,
        x: f64,
        y: f64,
    ) -> Option<(f64, f64, u32, u32)> {
        self.refresh_surface_origin_cache();
        let root_index = self
            .renderable_surfaces
            .iter()
            .position(|surface| surface.surface_id == root_surface_id)?;
        let root_origin = self.surface_origin_cache.get(root_index).copied()?;
        let geometry = self.current_root_window_geometry(root_surface_id)?;
        let window_geometry = self
            .surface_window_geometries
            .get(&root_surface_id)
            .copied();
        let local_x = x
            - f64::from(root_origin.0)
            - f64::from(
                window_geometry
                    .map(|geometry| geometry.x)
                    .unwrap_or_default(),
            );
        let local_y = y
            - f64::from(root_origin.1)
            - f64::from(
                window_geometry
                    .map(|geometry| geometry.y)
                    .unwrap_or_default(),
            );
        Some((local_x, local_y, geometry.width, geometry.height))
    }

    fn pointer_target_at(&mut self, x: f64, y: f64) -> Option<PointerTarget> {
        self.refresh_surface_origin_cache();
        let origins = &self.surface_origin_cache;
        for (index, renderable) in self.renderable_surfaces.iter().enumerate().rev() {
            let Some(origin) = origins.get(index).copied() else {
                continue;
            };
            let Some((surface_x, surface_y)) =
                render::surface_local_point_at_origin(renderable, origin, x, y)
            else {
                continue;
            };
            if !self.surface_accepts_input_at(renderable, surface_x, surface_y) {
                continue;
            }
            let Some(surface) = self.surface_resource_by_id(renderable.surface_id) else {
                continue;
            };

            return Some(PointerTarget {
                surface,
                surface_x,
                surface_y,
            });
        }

        if self.renderable_surfaces.is_empty() {
            self.focused_surface.clone().map(|surface| PointerTarget {
                surface,
                surface_x: x,
                surface_y: y,
            })
        } else {
            None
        }
    }

    fn pointer_target_for_surface_at_output(
        &mut self,
        surface: &wl_surface::WlSurface,
        x: f64,
        y: f64,
    ) -> Option<PointerTarget> {
        let surface_id = compositor_surface_id(surface);
        self.refresh_surface_origin_cache();
        let index = self
            .renderable_surfaces
            .iter()
            .position(|renderable| renderable.surface_id == surface_id)?;
        let renderable = &self.renderable_surfaces[index];
        let origin = self.surface_origin_cache.get(index).copied()?;
        let (surface_x, surface_y) =
            render::surface_local_point_at_origin(renderable, origin, x, y)?;
        Some(PointerTarget {
            surface: surface.clone(),
            surface_x,
            surface_y,
        })
    }

    fn surface_accepts_input_at(
        &self,
        surface: &RenderableSurface,
        surface_x: f64,
        surface_y: f64,
    ) -> bool {
        self.surface_resource_by_id(surface.surface_id)
            .and_then(|resource| {
                resource.data::<SurfaceData>().map(|data| {
                    data.input_region_contains(surface_x, surface_y, surface.width, surface.height)
                })
            })
            .unwrap_or(true)
    }

    fn refresh_pointer_focus_at_last_position(&mut self) {
        if self.active_locked_pointer_binding().is_some() {
            if let Some(active) = self.active_locked_pointer_binding() {
                self.pin_locked_pointer_focus(&active);
            }
            return;
        }

        let Some(target) = self.pointer_target_at(self.last_pointer_x, self.last_pointer_y) else {
            self.clear_pointer_focus();
            pointer_debug_log("post-unlock focus target=none");
            return;
        };

        pointer_debug_log(format!(
            "post-unlock focus target={} x={} y={}",
            compositor_surface_id(&target.surface),
            target.surface_x,
            target.surface_y
        ));
        self.ensure_pointer_focus(&target.surface);
        self.send_pointer_enter_if_needed(&target);
    }

    fn refresh_pointer_focus_after_implicit_grab(&mut self, old_surface_id: Option<u32>) {
        if self.active_locked_pointer_binding().is_some() {
            self.refresh_pointer_focus_at_last_position();
            return;
        }

        let target = self.pointer_target_at(self.last_pointer_x, self.last_pointer_y);
        let new_surface_id = target
            .as_ref()
            .map(|target| compositor_surface_id(&target.surface));
        pointer_debug_log(format!(
            "post-grab focus surface={} -> {}",
            old_surface_id
                .map(|surface_id| surface_id.to_string())
                .unwrap_or_else(|| "none".to_string()),
            new_surface_id
                .map(|surface_id| surface_id.to_string())
                .unwrap_or_else(|| "none".to_string())
        ));
        let Some(target) = target else {
            self.clear_pointer_focus();
            return;
        };
        if !self.pointer_target_allowed_by_popup_grab(&target) {
            self.clear_pointer_focus();
            return;
        }
        self.ensure_pointer_focus(&target.surface);
        self.send_pointer_enter_if_needed(&target);
    }

    fn restore_locked_pointer_position(
        &mut self,
        surface: &wl_surface::WlSurface,
        cursor_position_hint: Option<(f64, f64)>,
    ) -> Option<OutputPosition> {
        if let Some((surface_x, surface_y)) = cursor_position_hint {
            if !surface_x.is_finite() || !surface_y.is_finite() {
                pointer_debug_log(format!(
                    "pointer.unlock restore_source=committed_hint ignored reason=non_finite hint=({},{})",
                    surface_x, surface_y
                ));
            } else if let Some((output_x, output_y)) =
                self.output_position_for_valid_cursor_hint(surface, surface_x, surface_y)
            {
                self.last_pointer_x = output_x;
                self.last_pointer_y = output_y;
                pointer_debug_log(format!(
                    "pointer.unlock restore_source=committed_hint hint=({surface_x},{surface_y}) restore_output=({output_x},{output_y})"
                ));
                return Some(OutputPosition {
                    x: output_x,
                    y: output_y,
                });
            } else {
                pointer_debug_log(format!(
                    "pointer.unlock restore_source=committed_hint ignored reason=unresolved hint=({surface_x},{surface_y})"
                ));
            }
        }

        let fallback_position = self
            .active_locked_pointer_routing
            .as_ref()
            .filter(|active| same_surface_resource(&active.surface, surface))
            .map(|active| active.activation_anchor);
        let Some(position) = fallback_position else {
            pointer_debug_log("pointer.unlock restore_source=none restore_output=unchanged");
            return None;
        };
        self.last_pointer_x = position.x;
        self.last_pointer_y = position.y;
        pointer_debug_log(format!(
            "pointer.unlock restore_source=activation_anchor restore_output=({},{})",
            position.x, position.y
        ));
        Some(position)
    }

    fn output_position_for_valid_cursor_hint(
        &mut self,
        surface: &wl_surface::WlSurface,
        surface_x: f64,
        surface_y: f64,
    ) -> Option<(f64, f64)> {
        let surface_id = compositor_surface_id(surface);
        self.refresh_surface_origin_cache();
        let index = self
            .renderable_surfaces
            .iter()
            .position(|renderable| renderable.surface_id == surface_id)?;
        let renderable = &self.renderable_surfaces[index];
        if surface_x < 0.0
            || surface_y < 0.0
            || surface_x >= f64::from(renderable.width)
            || surface_y >= f64::from(renderable.height)
        {
            pointer_debug_log(format!(
                "pointer.unlock restore_source=committed_hint ignored reason=out_of_bounds hint=({},{}) size={}x{}",
                surface_x, surface_y, renderable.width, renderable.height
            ));
            return None;
        }
        let origin = self.surface_origin_cache.get(index).copied()?;
        Some((
            f64::from(origin.0) + surface_x,
            f64::from(origin.1) + surface_y,
        ))
    }

    fn surface_resource_by_id(&self, surface_id: u32) -> Option<wl_surface::WlSurface> {
        self.surface_resources.get(&surface_id).cloned()
    }

    fn ensure_pointer_focus(&mut self, surface: &wl_surface::WlSurface) {
        if let Some(active) = self.active_locked_pointer_binding()
            && !same_surface_resource(&active.surface, surface)
        {
            pointer_debug_log(format!(
                "pointer focus change suppressed by locked route id={} locked_surface={} requested={}",
                active.constraint_id,
                compositor_surface_id(&active.surface),
                compositor_surface_id(surface)
            ));
            self.pin_locked_pointer_focus(&active);
            return;
        }
        if let Some(active) = self.active_confined_pointer_binding()
            && !same_surface_resource(&active.surface, surface)
        {
            self.pin_confined_pointer_focus(&active);
            return;
        }
        if self
            .pointer_surface
            .as_ref()
            .is_some_and(|current| same_surface_resource(current, surface))
        {
            return;
        }

        self.clear_pointer_focus();
        self.pointer_surface = Some(surface.clone());
    }

    fn pointer_resource_entered_surface(
        &self,
        pointer: &wl_pointer::WlPointer,
        surface: &wl_surface::WlSurface,
    ) -> bool {
        self.pointer_entered_surfaces
            .iter()
            .any(|(resource, entered_surface)| {
                same_wayland_resource(resource, pointer)
                    && same_surface_resource(entered_surface, surface)
            })
    }

    fn pointer_has_current_enter_serial(
        &self,
        pointer: &wl_pointer::WlPointer,
        serial: u32,
        surface: &wl_surface::WlSurface,
    ) -> bool {
        self.pointer_enter_serials.iter().any(|entry| {
            same_wayland_resource(&entry.pointer, pointer)
                && same_surface_resource(&entry.surface, surface)
                && entry.serial == serial
        })
    }

    fn pointer_has_current_enter_serial_for_client(
        &self,
        pointer: &wl_pointer::WlPointer,
        serial: u32,
        surface: &wl_surface::WlSurface,
    ) -> bool {
        resource_belongs_to_surface_client(pointer, surface)
            && self.has_recent_input_serial_for_surface(serial, surface)
    }

    fn warp_pointer_protocol_request(
        &mut self,
        surface: wl_surface::WlSurface,
        pointer: wl_pointer::WlPointer,
        surface_x: f64,
        surface_y: f64,
        serial: u32,
    ) {
        let reject = |reason: &str| {
            pointer_debug_log(format!(
                "pointer_warp rejected pointer={} surface={} serial={} local=({},{}) reason={}",
                pointer.id().protocol_id(),
                compositor_surface_id(&surface),
                serial,
                surface_x,
                surface_y,
                reason
            ));
        };
        if !pointer.is_alive() || !surface.is_alive() {
            reject("dead_resource");
            return;
        }
        if !surface_x.is_finite() || !surface_y.is_finite() {
            reject("non_finite");
            return;
        }
        if !resource_belongs_to_surface_client(&pointer, &surface) {
            reject("wrong_client_pointer");
            return;
        }
        if !self
            .pointer_resources
            .iter()
            .any(|resource| same_wayland_resource(resource, &pointer))
        {
            reject("unknown_pointer");
            return;
        }
        let focused_surface = self
            .implicit_pointer_grab
            .as_ref()
            .map(|grab| grab.surface.clone())
            .or_else(|| self.pointer_surface.clone());
        let Some(focused_surface) = focused_surface else {
            reject("no_pointer_focus");
            return;
        };
        if !same_surface_resource(&focused_surface, &surface) {
            reject("surface_not_focused");
            return;
        }
        if !self.pointer_has_current_enter_serial_for_client(&pointer, serial, &surface) {
            reject("invalid_serial");
            return;
        }
        let Some(position) =
            self.valid_cursor_hint_output_position(&surface, Some((surface_x, surface_y)))
        else {
            reject("out_of_surface");
            return;
        };
        pointer_debug_log(format!(
            "pointer_warp accepted pointer={} serial={} local=({},{}) output=({},{}) matches_pending_unlock={}",
            pointer.id().protocol_id(),
            serial,
            surface_x,
            surface_y,
            position.x,
            position.y,
            self.pending_locked_pointer_reveal_matches(&pointer, &surface)
        ));
        let matches_pending_unlock = self.pending_locked_pointer_reveal_matches(&pointer, &surface);
        self.apply_pointer_warp(position, true);
        if matches_pending_unlock {
            if let Some(pending) = self.pending_locked_pointer_reveal.as_mut() {
                pending.fallback_position = Some(position);
            }
            self.finalize_pending_locked_pointer_reveal("matching_client_warp");
        }
    }

    fn remember_pointer_enter_serial(
        &mut self,
        pointer: &wl_pointer::WlPointer,
        surface: &wl_surface::WlSurface,
        serial: u32,
    ) {
        self.pointer_enter_serials
            .retain(|entry| !same_wayland_resource(&entry.pointer, pointer));
        self.pointer_enter_serials.push(PointerEnterSerial {
            pointer: pointer.clone(),
            surface: surface.clone(),
            serial,
        });
    }

    fn forget_pointer_enter_serial(&mut self, pointer: &wl_pointer::WlPointer) {
        self.pointer_enter_serials
            .retain(|entry| !same_wayland_resource(&entry.pointer, pointer));
    }

    fn synchronize_pointer_resource_focus(&mut self, pointer: &wl_pointer::WlPointer) -> bool {
        let Some(focused_surface) = self.pointer_surface.clone() else {
            return false;
        };
        if !pointer.is_alive() || !resource_belongs_to_surface_client(pointer, &focused_surface) {
            return false;
        }
        if self.pointer_resource_entered_surface(pointer, &focused_surface) {
            return true;
        }
        let Some(target) = self.pointer_target_at(self.last_pointer_x, self.last_pointer_y) else {
            return false;
        };
        if !same_surface_resource(&target.surface, &focused_surface) {
            return false;
        }
        self.send_pointer_enter_to_resource(pointer, &target);
        true
    }

    fn send_pointer_enter_to_resource(
        &mut self,
        pointer: &wl_pointer::WlPointer,
        target: &PointerTarget,
    ) {
        if let Some(index) = self
            .pointer_entered_surfaces
            .iter()
            .position(|(resource, _)| same_wayland_resource(resource, pointer))
        {
            if same_surface_resource(&self.pointer_entered_surfaces[index].1, &target.surface) {
                return;
            }

            let (_, previous_surface) = self.pointer_entered_surfaces.remove(index);
            self.forget_pointer_enter_serial(pointer);
            if resource_belongs_to_surface_client(pointer, &previous_surface) {
                let serial = self.next_configure_serial();
                let _ = pointer.send_event(wl_pointer::Event::Leave {
                    serial,
                    surface: previous_surface,
                });
                send_pointer_frame_if_supported(pointer);
            }
        }

        let serial = self.next_configure_serial();
        let _ = pointer.send_event(wl_pointer::Event::Enter {
            serial,
            surface: target.surface.clone(),
            surface_x: target.surface_x,
            surface_y: target.surface_y,
        });
        pointer_debug_log(format!(
            "wl_pointer {} synchronized enter for surface {}",
            pointer.id().protocol_id(),
            compositor_surface_id(&target.surface)
        ));
        self.remember_input_serial(serial, target.surface.clone());
        self.remember_pointer_enter_serial(pointer, &target.surface, serial);
        send_pointer_frame_if_supported(pointer);
        self.pointer_entered_surfaces
            .push((pointer.clone(), target.surface.clone()));
    }

    fn send_pointer_enter_if_needed(&mut self, target: &PointerTarget) {
        self.pointer_resources.retain(Resource::is_alive);
        let pointers = self
            .pointer_resources
            .iter()
            .filter(|pointer| resource_belongs_to_surface_client(*pointer, &target.surface))
            .cloned()
            .collect::<Vec<_>>();

        for pointer in pointers {
            self.send_pointer_enter_to_resource(&pointer, target);
        }
        let surface_id = compositor_surface_id(&target.surface);
        let constraint_ids = self
            .pointer_constraints
            .values()
            .filter(|constraint| compositor_surface_id(&constraint.surface) == surface_id)
            .map(|constraint| constraint.id)
            .collect::<Vec<_>>();
        for constraint_id in constraint_ids {
            self.maybe_request_pointer_constraint_activation(constraint_id);
        }
    }

    fn clear_pointer_focus(&mut self) {
        if let Some(active) = self.active_locked_pointer_binding() {
            pointer_debug_log(format!(
                "pointer focus clear suppressed by locked route id={} surface={}",
                active.constraint_id,
                compositor_surface_id(&active.surface)
            ));
            self.pin_locked_pointer_focus(&active);
            return;
        }
        if let Some(active) = self.active_confined_pointer_binding() {
            pointer_debug_log(format!(
                "pointer focus clear suppressed by confined route id={} surface={}",
                active.constraint_id,
                compositor_surface_id(&active.surface)
            ));
            self.pin_confined_pointer_focus(&active);
            return;
        }
        if let Some(surface_id) = self.pointer_surface.as_ref().map(compositor_surface_id) {
            pointer_debug_log(format!(
                "pointer focus loss deactivating constraints surface={}",
                surface_id
            ));
            self.deactivate_pointer_constraints_for_surface_focus_loss(surface_id, true);
        }
        let cleared_client_cursor = self.active_client_cursor.take().is_some();
        self.cursor_visibility.client_hidden_pointer = None;
        self.cursor_visibility.client_cursor_pointer = None;
        if cleared_client_cursor {
            self.advance_render_generation(RenderGenerationCause::CursorState);
            pointer_debug_log("cursor cleanup reason=pointer-focus-loss");
        }
        self.sync_cursor_visibility_request();
        self.pointer_surface = None;
        self.pointer_resources.retain(Resource::is_alive);
        let pointers = self.pointer_resources.clone();
        for pointer in pointers {
            let Some(index) = self
                .pointer_entered_surfaces
                .iter()
                .position(|(resource, _)| same_wayland_resource(resource, &pointer))
            else {
                continue;
            };
            let (_, surface) = self.pointer_entered_surfaces.remove(index);
            self.forget_pointer_enter_serial(&pointer);
            if !resource_belongs_to_surface_client(&pointer, &surface) {
                continue;
            }
            let serial = self.next_configure_serial();
            let _ = pointer.send_event(wl_pointer::Event::Leave { serial, surface });
            send_pointer_frame_if_supported(&pointer);
        }
    }

    fn complete_frame_callbacks_now(&mut self, data: &SurfaceData) {
        let callbacks = data.take_frame_callbacks();
        self.complete_frame_callbacks(callbacks);
    }

    fn complete_pending_frame_callbacks(&mut self) {
        let mut callbacks = std::mem::take(&mut self.pending_frame_callbacks);
        for surface in self.surface_resources.values() {
            if let Some(data) = surface.data::<SurfaceData>() {
                callbacks.extend(data.take_frame_callbacks());
            }
        }
        self.complete_frame_callbacks(callbacks);
    }

    fn has_pending_frame_callbacks(&self) -> bool {
        !self.pending_frame_callbacks.is_empty()
            || self.pending_explicit_sync_commits.iter().any(|commit| {
                !self.external_acquire_readiness && !commit.frame_callbacks.is_empty()
            })
            || self
                .surface_resources
                .values()
                .filter_map(Resource::data::<SurfaceData>)
                .any(SurfaceData::has_frame_callbacks)
    }

    fn has_only_pending_surface_frame_callbacks(&self) -> bool {
        !self.pending_resize_configure_is_flushable()
            && self.pending_frame_callbacks.is_empty()
            && self.pending_explicit_sync_commits.is_empty()
            && self.pending_presentation_feedbacks.is_empty()
            && self
                .surface_resources
                .values()
                .filter_map(Resource::data::<SurfaceData>)
                .any(SurfaceData::has_frame_callbacks)
    }

    fn has_pending_frame_prepare_work(&self) -> bool {
        self.pending_resize_configure_is_flushable()
            || self.pending_explicit_sync_commits.iter().any(|commit| {
                !self.external_acquire_readiness
                    || commit.acquire_state == PendingAcquireState::Ready
            })
            || !self.pending_color_info.is_empty()
    }

    fn has_pending_explicit_sync_work(&self) -> bool {
        !self.pending_explicit_sync_commits.is_empty()
    }

    fn has_pending_frame_work(&self) -> bool {
        self.pending_resize_configure_is_flushable()
            || self.has_pending_frame_callbacks()
            || !self.pending_presentation_feedbacks.is_empty()
    }

    fn complete_pending_presentation_feedbacks(&mut self, presentation: FramePresentation) {
        let feedbacks = std::mem::take(&mut self.pending_presentation_feedbacks);
        if feedbacks.is_empty() {
            return;
        }

        let timestamp = presentation.timestamp;
        let (tv_sec_hi, tv_sec_lo) = timestamp.protocol_seconds();
        let sequence = presentation.sequence;
        let flags = match presentation.kind {
            PresentationKind::Synchronized => wp_presentation_feedback::Kind::Vsync,
            PresentationKind::Software => wp_presentation_feedback::Kind::empty(),
        };
        for pending in feedbacks {
            if !pending.surface.is_alive() || presentation.clock != self.presentation_clock {
                pending.feedback.discarded();
                continue;
            }
            for output in self
                .output_resources
                .iter()
                .filter(|output| resource_belongs_to_surface_client(*output, &pending.surface))
            {
                pending.feedback.sync_output(output);
            }
            pending.feedback.presented(
                tv_sec_hi,
                tv_sec_lo,
                timestamp.nanoseconds(),
                self.output_refresh.presentation_refresh_nsec(),
                (sequence >> 32) as u32,
                sequence as u32,
                flags,
            );
        }
    }

    fn discard_pending_presentation_feedbacks_for_surface(&mut self, surface_id: u32) {
        let mut pending_feedbacks = Vec::new();
        for pending in std::mem::take(&mut self.pending_presentation_feedbacks) {
            if pending.surface_id == surface_id {
                pending.feedback.discarded();
            } else {
                pending_feedbacks.push(pending);
            }
        }
        self.pending_presentation_feedbacks = pending_feedbacks;
    }

    fn discard_all_pending_presentation_feedbacks(&mut self) {
        for pending in std::mem::take(&mut self.pending_presentation_feedbacks) {
            pending.feedback.discarded();
        }
    }

    fn release_pending_buffers(&mut self) {
        let buffers = std::mem::take(&mut self.pending_buffer_releases);
        for buffer in buffers {
            let _ = buffer.send_event(wl_buffer::Event::Release);
        }

        let dmabuf_releases = std::mem::replace(
            &mut self.deferred_dmabuf_buffer_releases,
            std::mem::take(&mut self.pending_dmabuf_buffer_releases),
        );
        for release in dmabuf_releases {
            release.release();
        }
    }

    fn complete_frame_callbacks(&mut self, callbacks: Vec<wl_callback::WlCallback>) {
        let time = self.frame_callback_time_ms();
        for callback in callbacks {
            let _ = callback.send_event(wl_callback::Event::Done {
                callback_data: time,
            });
        }
    }

    fn cancel_pending_acquire_commits_for_surface(
        &mut self,
        surface_id: u32,
        reason: AcquireWatchCancelReason,
    ) -> Vec<wl_callback::WlCallback> {
        let mut retained = Vec::with_capacity(self.pending_explicit_sync_commits.len());
        let mut canceled_callbacks = Vec::new();
        let mut canceled_resize_captures = Vec::new();
        for commit in std::mem::take(&mut self.pending_explicit_sync_commits) {
            if commit.surface_id == surface_id {
                canceled_callbacks.extend(commit.frame_callbacks);
                if let Some(resize) = commit.pending.resize_commit.as_deref() {
                    canceled_resize_captures.push(resize.commit_sequence);
                }
                if self.external_acquire_readiness {
                    self.pending_acquire_watch_changes
                        .push(AcquireWatchChange::Cancel {
                            commit_id: commit.commit_id,
                            reason,
                        });
                }
            } else {
                retained.push(commit);
            }
        }
        self.pending_explicit_sync_commits = retained;
        if let Some(flow) = self.resize_configure_flows.get_mut(&surface_id) {
            for commit_sequence in canceled_resize_captures {
                flow.release_capture(commit_sequence);
            }
        }
        canceled_callbacks
    }

    fn retain_oldest_pending_acquire_for_surface(
        &mut self,
        surface_id: u32,
    ) -> Vec<wl_callback::WlCallback> {
        let mut retained = Vec::with_capacity(self.pending_explicit_sync_commits.len());
        let mut kept_oldest = false;
        let mut superseded_callbacks = Vec::new();
        let mut released_captures = Vec::new();
        for commit in std::mem::take(&mut self.pending_explicit_sync_commits) {
            if commit.surface_id != surface_id || !kept_oldest {
                kept_oldest |= commit.surface_id == surface_id;
                retained.push(commit);
                continue;
            }
            superseded_callbacks.extend(commit.frame_callbacks);
            if let Some(resize) = commit.pending.resize_commit.as_deref() {
                released_captures.push(resize.commit_sequence);
            }
            if self.external_acquire_readiness {
                self.pending_acquire_watch_changes
                    .push(AcquireWatchChange::Cancel {
                        commit_id: commit.commit_id,
                        reason: AcquireWatchCancelReason::Superseded,
                    });
            }
        }
        self.pending_explicit_sync_commits = retained;
        if let Some(flow) = self.resize_configure_flows.get_mut(&surface_id) {
            for commit_sequence in released_captures {
                flow.release_capture(commit_sequence);
            }
        }
        superseded_callbacks
    }

    fn cancel_pending_acquire_commits_for_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        reason: AcquireWatchCancelReason,
    ) {
        let ids = self
            .pending_explicit_sync_commits
            .iter()
            .filter(|commit| same_wayland_resource(&commit.pending.resource, buffer))
            .map(|commit| commit.surface_id)
            .collect::<Vec<_>>();
        for surface_id in ids {
            self.cancel_pending_acquire_commits_for_surface(surface_id, reason);
        }
    }

    fn cancel_pending_acquire_commits_for_timeline(
        &mut self,
        timeline: &crate::syncobj::DrmSyncobjTimeline,
        reason: AcquireWatchCancelReason,
    ) {
        let mut retained = Vec::with_capacity(self.pending_explicit_sync_commits.len());
        let mut released_captures = Vec::new();
        for commit in std::mem::take(&mut self.pending_explicit_sync_commits) {
            let uses_timeline = commit.acquire.timeline.same_timeline(timeline)
                || commit
                    .pending
                    .explicit_release
                    .as_ref()
                    .is_some_and(|release| release.timeline.same_timeline(timeline));
            if uses_timeline {
                if let Some(resize) = commit.pending.resize_commit.as_deref() {
                    released_captures.push((commit.surface_id, resize.commit_sequence));
                }
                if self.external_acquire_readiness {
                    self.pending_acquire_watch_changes
                        .push(AcquireWatchChange::Cancel {
                            commit_id: commit.commit_id,
                            reason,
                        });
                }
            } else {
                retained.push(commit);
            }
        }
        self.pending_explicit_sync_commits = retained;
        for (surface_id, commit_sequence) in released_captures {
            if let Some(flow) = self.resize_configure_flows.get_mut(&surface_id) {
                flow.release_capture(commit_sequence);
            }
        }
    }

    fn enable_external_acquire_readiness(&mut self) {
        if self.external_acquire_readiness {
            return;
        }
        self.external_acquire_readiness = true;
        for commit in &self.pending_explicit_sync_commits {
            if commit.acquire_state == PendingAcquireState::Ready {
                continue;
            }
            self.pending_acquire_watch_changes
                .push(AcquireWatchChange::Register(AcquireWatchRequest {
                    commit_id: commit.commit_id,
                    surface_id: commit.surface_id,
                    buffer_id: commit.pending.resource.id().protocol_id(),
                    acquire: commit.acquire.clone(),
                    received_at: Instant::now(),
                }));
        }
    }

    fn take_acquire_watch_changes(&mut self) -> Vec<AcquireWatchChange> {
        std::mem::take(&mut self.pending_acquire_watch_changes)
    }

    fn mark_acquire_commit_eventfd_backed(&mut self, commit_id: AcquireCommitId) -> bool {
        self.pending_explicit_sync_commits
            .iter_mut()
            .find(|commit| commit.commit_id == commit_id)
            .is_some_and(|commit| commit.acquire_state.mark_eventfd_backed())
    }

    fn mark_acquire_commit_fallback_backed(&mut self, commit_id: AcquireCommitId) -> bool {
        self.pending_explicit_sync_commits
            .iter_mut()
            .find(|commit| commit.commit_id == commit_id)
            .is_some_and(|commit| commit.acquire_state.mark_fallback_backed())
    }

    fn mark_acquire_commit_ready(
        &mut self,
        commit_id: AcquireCommitId,
        surface_id: u32,
        acquire: &ExplicitSyncPoint,
    ) -> bool {
        self.pending_explicit_sync_commits
            .iter_mut()
            .find(|commit| {
                commit.commit_id == commit_id
                    && commit.surface_id == surface_id
                    && commit.acquire == *acquire
            })
            .is_some_and(|commit| commit.acquire_state.mark_ready())
    }

    fn commit_ready_explicit_sync_buffers(&mut self) {
        let mut commits = std::mem::take(&mut self.pending_explicit_sync_commits);
        for commit in &mut commits {
            if !self.external_acquire_readiness && commit.acquire.is_signaled() {
                commit.acquire_state.mark_ready();
            }
        }
        let newest_ready = newest_ready_explicit_sync_commit_indices(
            commits.iter().enumerate().map(|(index, commit)| {
                (
                    index,
                    commit.surface_id,
                    commit.acquire_state == PendingAcquireState::Ready,
                )
            }),
        );

        let mut waiting = Vec::new();
        let mut ready = Vec::new();
        let mut superseded_callbacks: HashMap<u32, Vec<wl_callback::WlCallback>> = HashMap::new();
        let mut released_captures = Vec::new();
        for (index, commit) in commits.into_iter().enumerate() {
            let Some(&ready_index) = newest_ready.get(&commit.surface_id) else {
                waiting.push(commit);
                continue;
            };
            if index < ready_index {
                superseded_callbacks
                    .entry(commit.surface_id)
                    .or_default()
                    .extend(commit.frame_callbacks);
                if let Some(resize) = commit.pending.resize_commit.as_deref() {
                    released_captures.push((commit.surface_id, resize.commit_sequence));
                }
                if self.external_acquire_readiness {
                    self.pending_acquire_watch_changes
                        .push(AcquireWatchChange::Cancel {
                            commit_id: commit.commit_id,
                            reason: AcquireWatchCancelReason::Superseded,
                        });
                }
            } else if index == ready_index {
                ready.push(commit);
            } else {
                waiting.push(commit);
            }
        }
        self.pending_explicit_sync_commits = waiting;
        for (surface_id, commit_sequence) in released_captures {
            if let Some(flow) = self.resize_configure_flows.get_mut(&surface_id) {
                flow.release_capture(commit_sequence);
            }
        }
        for mut commit in ready {
            let mut callbacks = superseded_callbacks
                .remove(&commit.surface_id)
                .unwrap_or_default();
            callbacks.extend(commit.frame_callbacks);
            if commit.pending.resize_commit.is_none() {
                commit.pending.resize_commit = self
                    .capture_acked_resize_for_surface_commit(commit.surface_id)
                    .map(|snapshot| {
                        self.snapshot_resize_commit_for_buffer(
                            commit.surface_id,
                            snapshot,
                            &commit.pending,
                        )
                    })
                    .map(Box::new);
            }
            self.commit_surface_buffer_by_role(
                commit.surface_id,
                commit.pending,
                commit.damage,
                callbacks,
            );
        }
    }
}

fn newest_ready_explicit_sync_commit_indices(
    commits: impl IntoIterator<Item = (usize, u32, bool)>,
) -> HashMap<u32, usize> {
    let mut newest_ready = HashMap::new();
    for (index, surface_id, ready) in commits {
        if ready {
            newest_ready.insert(surface_id, index);
        }
    }
    newest_ready
}

fn damage_only_rendered_surface_size(
    existing: BufferSize,
    requested: BufferSize,
    resize_pending: bool,
) -> BufferSize {
    if resize_pending { existing } else { requested }
}

fn resource_belongs_to_surface_client<R>(resource: &R, surface: &wl_surface::WlSurface) -> bool
where
    R: Resource,
{
    resource.id().same_client_as(&surface.id())
}

fn same_wayland_resource<L, R>(left: &L, right: &R) -> bool
where
    L: Resource,
    R: Resource,
{
    left.id().protocol_id() == right.id().protocol_id() && left.id().same_client_as(&right.id())
}

fn same_surface_resource(left: &wl_surface::WlSurface, right: &wl_surface::WlSurface) -> bool {
    same_wayland_resource(left, right)
}

fn same_buffer_resource(left: &wl_buffer::WlBuffer, right: &wl_buffer::WlBuffer) -> bool {
    same_wayland_resource(left, right)
}

fn normalize_selection_mime_types(mime_types: Vec<String>) -> Vec<String> {
    const MAX_SOURCE_MIME_TYPES: usize = 128;
    const MAX_MIME_TYPE_LEN: usize = 4096;
    let mut normalized = Vec::new();
    for mime_type in mime_types {
        if mime_type.is_empty()
            || mime_type.len() > MAX_MIME_TYPE_LEN
            || normalized.iter().any(|existing| existing == &mime_type)
        {
            continue;
        }
        normalized.push(mime_type);
        if normalized.len() >= MAX_SOURCE_MIME_TYPES {
            break;
        }
    }
    normalized
}

#[allow(clippy::too_many_arguments)]
fn update_renderable_surface_buffer(
    surface: &mut RenderableSurface,
    pending: &PendingSurfaceBuffer,
    buffer_size: BufferSize,
    width: u32,
    height: u32,
    placement: SurfacePlacement,
    generation: u64,
    damage: RenderableSurfaceDamage,
) -> io::Result<()> {
    let previous_preview = surface.resize_preview;
    let previous_visual = previous_preview
        .map(|_| WindowGeometry::new(surface.placement, surface.width, surface.height));
    if pending.data.is_shm()
        && surface.buffer_size() == buffer_size
        && surface.buffer_id() == pending.data.buffer_id()
        && let Some(pixels) = surface.shm_pixels_mut()
    {
        pending.data.read_pixels_into_with_damage(pixels, &damage)?;
    } else {
        surface.buffer = pending.data.to_committed_buffer_for_size(buffer_size)?;
    }
    surface.x = pending.x;
    surface.y = pending.y;
    surface.width = width;
    surface.height = height;
    surface.placement = placement;
    surface.resize_preview = None;
    surface.generation = generation;
    surface.damage = damage;
    if let Some(visual) = previous_visual
        && (visual.width != width || visual.height != height || visual.placement != placement)
    {
        surface.width = visual.width;
        surface.height = visual.height;
        surface.placement = visual.placement;
        surface.resize_preview = Some(ResizePreview {
            committed_width: width,
            committed_height: height,
            anchor_right: previous_preview.is_some_and(|preview| preview.anchor_right),
            anchor_bottom: previous_preview.is_some_and(|preview| preview.anchor_bottom),
        });
    }
    Ok(())
}

fn root_surface_id_for_surface_in_placements(
    placements: &HashMap<u32, SurfacePlacement>,
    surface_id: u32,
) -> u32 {
    let mut current = surface_id;
    for _ in 0..placements.len().saturating_add(1) {
        let Some(parent) = placements
            .get(&current)
            .copied()
            .unwrap_or_default()
            .parent_surface_id
            .filter(|parent_id| *parent_id != current)
        else {
            return current;
        };
        current = parent;
    }

    surface_id
}

#[cfg(test)]
mod tests;
