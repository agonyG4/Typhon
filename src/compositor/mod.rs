use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io,
    os::fd::OwnedFd,
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
    fractional_scale::v1::server::{wp_fractional_scale_manager_v1, wp_fractional_scale_v1},
    idle_inhibit::zv1::server::{zwp_idle_inhibit_manager_v1, zwp_idle_inhibitor_v1},
    pointer_constraints::zv1::server::{
        zwp_confined_pointer_v1, zwp_locked_pointer_v1, zwp_pointer_constraints_v1,
    },
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
    protocol::{
        wl_buffer, wl_callback, wl_compositor, wl_data_device, wl_data_device_manager,
        wl_data_source, wl_keyboard, wl_output, wl_pointer, wl_region, wl_seat, wl_shm,
        wl_shm_pool, wl_subcompositor, wl_subsurface, wl_surface,
    },
};

use crate::render_backend::buffer::{
    BufferSize, DmabufBufferHandle, DmabufPlane as RenderDmabufPlane, DmabufPlaneDescriptor,
    DrmFormat, DrmModifier,
};
use crate::render_backend::egl_gles::EglGlesDmabufFeedback;
use crate::syncobj::DrmSyncobjDevice;
use crate::wayland_drm::server::wl_drm;

mod color;
mod dmabuf;
mod explicit_sync;
mod idle;
mod input;
mod interaction;
mod output;
mod plan;
mod popup;
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
use explicit_sync::{
    ExplicitSyncPoint, PendingExplicitSyncCommit, PendingPresentationFeedback,
    SYNCOBJ_MANAGER_ERROR_INVALID_TIMELINE, SYNCOBJ_MANAGER_ERROR_SURFACE_EXISTS,
    SYNCOBJ_SURFACE_ERROR_CONFLICTING_POINTS, SYNCOBJ_SURFACE_ERROR_NO_ACQUIRE_POINT,
    SYNCOBJ_SURFACE_ERROR_NO_BUFFER, SYNCOBJ_SURFACE_ERROR_NO_RELEASE_POINT,
    SYNCOBJ_SURFACE_ERROR_NO_SURFACE, SYNCOBJ_SURFACE_ERROR_UNSUPPORTED_BUFFER,
    SyncobjSurfaceState, SyncobjTimelineData, presentation_timestamp,
};
pub use idle::{IdleManager, IdleState};
use input::{
    InputSerial, KeyboardModifierState, send_keyboard_initial_state,
    send_pointer_frame_if_supported, wayland_event_time,
};
pub use input::{
    OutputPosition, PointerConstraintMode, PointerConstraintState, PointerMotionSample,
    RelativePointerMotion,
};
use interaction::{
    PendingResizeCommit, PendingResizeConfigure, PointerPress, PointerTarget, ResizeEdges,
    RootSurfaceHit, WindowFrameHit, WindowInteraction, WindowInteractionKind,
    interactive_resize_geometry, resize_drag_threshold_reached, resize_edges_for_window_point,
    resize_edges_from_xdg, window_frame_action_for_local_point,
};
use output::{
    OutputRefreshRate, OutputScale, OutputSize, send_output_description,
    send_output_done_if_supported, send_output_mode, send_output_scale,
};
pub use plan::{
    ArchitectureLayer, CompositorArchitecture, CompositorPlan, InputProtocolCapabilities,
    ProtocolGlobal,
};
use popup::{
    PopupAnchorRect, PopupConstraintAdjustment, PopupEdges, PopupRect, XdgPositionerState,
    XdgWindowGeometry,
};
pub use render::{
    BufferAge, DesktopComposeRequest, DesktopFrameCopyKind, DesktopSceneRebuildKind,
    DesktopSceneRenderer, DesktopVisualState, NESTED_OUTPUT_BACKGROUND, RenderSceneElement,
    RenderSceneElementId, RenderSceneElementKind, ServerFrameColor, SurfaceTargetRect,
    compose_nested_output, cursor_texture_pixels, cursor_texture_size, draw_wallpaper,
    output_scale_key, render_scene_elements_for_surfaces, scale_desktop_visual_state,
    scale_logical_coordinate, scale_logical_extent, server_frame_rects_by_surface,
    server_frame_rects_for_surface, surface_origin, surface_origins,
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

fn resize_commit_matches_size(resize: PendingResizeCommit, committed_size: BufferSize) -> bool {
    committed_size.width == resize.width && committed_size.height == resize.height
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
        }
    }

    pub const fn uses_surface_damage(self) -> bool {
        matches!(self, Self::SurfaceCommit | Self::SurfaceDamage)
    }
}

#[derive(Debug, Default)]
pub struct CompositorState {
    pub accepted_clients: usize,
    pub xdg_toplevels: usize,
    pub xdg_popups: usize,
    pub last_app_id: Option<String>,
    pub renderable_surfaces: Vec<RenderableSurface>,
    next_surface_id: u32,
    surface_resources: HashMap<u32, wl_surface::WlSurface>,
    output_resources: Vec<wl_output::WlOutput>,
    fractional_scale_resources: HashMap<u32, Vec<wp_fractional_scale_v1::WpFractionalScaleV1>>,
    keyboard_resources: Vec<wl_keyboard::WlKeyboard>,
    pointer_resources: Vec<wl_pointer::WlPointer>,
    relative_pointer_resources: Vec<zwp_relative_pointer_v1::ZwpRelativePointerV1>,
    idle_inhibitor_resources: Vec<zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1>,
    idle_manager: IdleManager,
    output_size: OutputSize,
    output_scale: OutputScale,
    output_refresh: OutputRefreshRate,
    focused_surface: Option<wl_surface::WlSurface>,
    keyboard_surface: Option<wl_surface::WlSurface>,
    keyboard_modifiers: KeyboardModifierState,
    pointer_surface: Option<wl_surface::WlSurface>,
    pointer_constraint: PointerConstraintState,
    pointer_entered_surfaces: Vec<(wl_pointer::WlPointer, wl_surface::WlSurface)>,
    cursor_surface_ids: HashSet<u32>,
    surface_placements: HashMap<u32, SurfacePlacement>,
    current_surface_buffers: HashMap<u32, PendingSurfaceBuffer>,
    surface_window_geometries: HashMap<u32, XdgWindowGeometry>,
    surface_entered_outputs: HashSet<(u32, u32)>,
    toplevel_surfaces: HashMap<u32, ToplevelSurface>,
    configured_xdg_surfaces: HashSet<u32>,
    window_interaction: Option<WindowInteraction>,
    pending_resize_configure: Option<PendingResizeConfigure>,
    sent_resize_commits: HashMap<(u32, u32), PendingResizeCommit>,
    pending_resize_commits: HashMap<u32, PendingResizeCommit>,
    last_pointer_x: f64,
    last_pointer_y: f64,
    last_pointer_motion_usec: Option<u64>,
    last_relative_pointer_motion: Option<RelativePointerMotion>,
    last_pointer_press: Option<PointerPress>,
    recent_input_serials: Vec<InputSerial>,
    active_dmabuf_buffers: HashMap<u32, SurfaceBufferRelease>,
    pending_buffer_releases: Vec<wl_buffer::WlBuffer>,
    pending_dmabuf_buffer_releases: Vec<SurfaceBufferRelease>,
    deferred_dmabuf_buffer_releases: Vec<SurfaceBufferRelease>,
    pending_explicit_sync_commits: Vec<PendingExplicitSyncCommit>,
    pending_frame_callbacks: Vec<wl_callback::WlCallback>,
    pending_presentation_feedbacks: Vec<PendingPresentationFeedback>,
    frame_clock_start: Option<Instant>,
    next_configure_serial: u32,
    render_generation: u64,
    render_generation_cause: RenderGenerationCause,
    surface_origin_cache_generation: Option<u64>,
    surface_origin_cache: Vec<(i32, i32)>,
    dmabuf_feedback: EglGlesDmabufFeedback,
    dmabuf_main_device: u64,
    dmabuf_main_device_path: Option<String>,
    syncobj_device: Option<DrmSyncobjDevice>,
    popup_surfaces: HashMap<u32, PopupSurface>,
    popup_grab_stack: Vec<u32>,
    pending_color_info: Vec<color::PendingColorInfo>,
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
            ..Self::default()
        }
    }

    fn next_render_generation_value(&self) -> u64 {
        self.render_generation.saturating_add(1)
    }

    fn set_render_generation(&mut self, generation: u64, cause: RenderGenerationCause) {
        self.render_generation = generation;
        self.render_generation_cause = cause;
    }

    fn advance_render_generation(&mut self, cause: RenderGenerationCause) -> u64 {
        let generation = self.next_render_generation_value();
        self.set_render_generation(generation, cause);
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
        self.focused_surface = Some(surface);
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
        self.discard_pending_presentation_feedbacks_for_surface(surface_id);
        self.surface_resources.remove(&surface_id);
        self.cursor_surface_ids.remove(&surface_id);
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
        for removed_surface_id in removed_surface_ids {
            self.popup_surfaces.remove(&removed_surface_id);
            if let Some(buffer) = self.active_dmabuf_buffers.remove(&removed_surface_id) {
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
            .is_some_and(|surface| compositor_surface_id(surface) == surface_id)
        {
            self.pointer_surface = None;
            self.clear_pointer_constraint();
        }
        self.pointer_entered_surfaces
            .retain(|(_, surface)| compositor_surface_id(surface) != surface_id);
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
    }

    fn register_pointer(&mut self, pointer: wl_pointer::WlPointer) {
        if self
            .pointer_resources
            .iter()
            .any(|resource| same_wayland_resource(resource, &pointer))
        {
            return;
        }
        self.pointer_resources.push(pointer);
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
    }

    fn set_pointer_cursor(&mut self, surface: Option<wl_surface::WlSurface>) {
        let Some(surface) = surface else {
            return;
        };
        let surface_id = compositor_surface_id(&surface);
        self.cursor_surface_ids.insert(surface_id);
        self.unmap_surface_content(surface_id);
    }

    fn is_cursor_surface(&self, surface_id: u32) -> bool {
        self.cursor_surface_ids.contains(&surface_id)
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
        self.keyboard_surface = Some(surface.clone());
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
    }

    fn send_pointer_motion(&mut self, x: f64, y: f64) {
        self.last_pointer_x = x;
        self.last_pointer_y = y;
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
    ) {
        self.relative_pointer_resources.push(pointer);
    }

    fn remove_relative_pointer_resource(
        &mut self,
        pointer: &zwp_relative_pointer_v1::ZwpRelativePointerV1,
    ) {
        self.relative_pointer_resources
            .retain(|resource| !same_wayland_resource(resource, pointer));
    }

    fn send_relative_pointer_motion(&mut self, timestamp_usec: u64, motion: RelativePointerMotion) {
        if motion.is_zero() {
            return;
        }
        let Some(surface) = self.pointer_surface.clone() else {
            return;
        };
        self.relative_pointer_resources.retain(Resource::is_alive);
        let utime_hi = (timestamp_usec >> 32) as u32;
        let utime_lo = (timestamp_usec & 0xffff_ffff) as u32;
        for pointer in self
            .relative_pointer_resources
            .iter()
            .filter(|pointer| resource_belongs_to_surface_client(*pointer, &surface))
        {
            pointer.relative_motion(
                utime_hi,
                utime_lo,
                motion.dx,
                motion.dy,
                motion.dx_unaccelerated,
                motion.dy_unaccelerated,
            );
        }
    }

    fn send_pointer_button(&mut self, button: u32, pressed: bool) {
        let target = self.pointer_target_at(self.last_pointer_x, self.last_pointer_y);
        if pressed
            && let Some(popup_surface_id) =
                self.popup_grab_to_dismiss_for_pointer_target(target.as_ref())
        {
            self.dismiss_popup_surface(popup_surface_id);
            let _ = self.focus_topmost_renderable_toplevel();
            return;
        }

        let implicit_grab_surface = (!pressed)
            .then(|| {
                self.last_pointer_press
                    .as_ref()
                    .filter(|press| press.button == button)
                    .map(|press| press.surface.clone())
            })
            .flatten();

        if implicit_grab_surface.is_none()
            && let Some(target) = target.as_ref()
        {
            self.ensure_pointer_focus(&target.surface);
            self.send_pointer_enter_if_needed(target);
        }

        let Some(surface) = implicit_grab_surface
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
            self.last_pointer_press = Some(PointerPress {
                serial,
                button,
                surface: surface.clone(),
                root_surface_id,
                output_x: self.last_pointer_x,
                output_y: self.last_pointer_y,
            });
        } else if self
            .last_pointer_press
            .as_ref()
            .is_some_and(|press| press.button == button)
        {
            self.last_pointer_press = None;
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
    }

    fn send_pointer_axis(&mut self, horizontal: f64, vertical: f64) {
        if horizontal == 0.0 && vertical == 0.0 {
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
        if let Some(surface) = self.surface_resource_by_id(surface_id) {
            self.ensure_surface_entered_outputs(&surface);
        }

        let generation = self.next_render_generation_value();
        let placement = match self.take_pending_resize_commit_placement(surface_id, &pending) {
            Ok(Some(placement)) => placement,
            Ok(None) => self.surface_placement(surface_id),
            Err(_) => return,
        };
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
                "oblivion-one compositor: commit surface {surface_id} buffer={}x{} surface={}x{} offset={},{} shm={} dmabuf={} pending_resize={:?}",
                buffer_width,
                buffer_height,
                width,
                height,
                pending.x,
                pending.y,
                pending.data.is_shm(),
                pending.data.is_dmabuf(),
                self.pending_resize_commits.get(&surface_id),
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
            return;
        }
        if let Some(existing) = self
            .renderable_surfaces
            .iter_mut()
            .find(|surface| surface.surface_id == surface_id)
        {
            let damage = if existing.buffer_size() == buffer_size {
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
        } else {
            let damage = damage.normalized_for_surface(buffer_width, buffer_height);
            let surface =
                match pending.to_renderable_surface(surface_id, placement, generation, damage) {
                    Ok(surface) => surface,
                    Err(_) => return,
                };
            self.renderable_surfaces.push(surface);
        }
        self.stack_renderable_descendants_above_parent(surface_id);

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
        let resize_pending = self.pending_resize_commits.contains_key(&surface_id);
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
                self.pending_resize_commits.get(&surface_id),
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
        if self.is_cursor_surface(surface_id) {
            self.commit_cursor_surface_buffer(surface_id, pending, frame_callbacks);
            return;
        }

        self.configure_xdg_surface_if_needed(surface_id);
        let Some(sync_state) = explicit_sync else {
            self.commit_surface_buffer(surface_id, pending, damage);
            self.pending_frame_callbacks.extend(frame_callbacks);
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
        if acquire.is_signaled() {
            self.commit_surface_buffer(surface_id, pending, damage);
            self.pending_frame_callbacks.extend(frame_callbacks);
        } else {
            self.pending_explicit_sync_commits
                .push(PendingExplicitSyncCommit {
                    surface_id,
                    pending,
                    damage,
                    frame_callbacks,
                    acquire,
                });
        }
    }

    fn commit_surface_without_buffer(
        &mut self,
        surface_id: u32,
        data: &SurfaceData,
        damage: Option<RenderableSurfaceDamage>,
        explicit_sync: Option<Arc<SyncobjSurfaceState>>,
    ) {
        if self.is_cursor_surface(surface_id) {
            let _ = data.commit_pending_viewport();
            let _ = data.commit_pending_buffer_scale();
            self.complete_frame_callbacks_now(data);
            return;
        }

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

        self.configure_xdg_surface_if_needed(surface_id);
        let surface_size = data.commit_pending_viewport();
        let buffer_scale = data.commit_pending_buffer_scale();
        if let Some(damage) = damage {
            self.commit_surface_damage_only(surface_id, damage, surface_size, buffer_scale);
        }
        self.complete_frame_callbacks_now(data);
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

    fn clear_resize_state_for_surfaces(&mut self, surface_ids: &[u32]) {
        if self
            .pending_resize_configure
            .is_some_and(|pending| surface_ids.contains(&pending.surface_id))
        {
            self.pending_resize_configure = None;
        }
        self.sent_resize_commits
            .retain(|(surface_id, _), _| !surface_ids.contains(surface_id));
        self.pending_resize_commits
            .retain(|surface_id, _| !surface_ids.contains(surface_id));
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
        frame_callbacks: Vec<wl_callback::WlCallback>,
    ) {
        self.unmap_surface_content(surface_id);
        self.track_committed_buffer_lifetime(surface_id, &pending);
        self.current_surface_buffers.insert(surface_id, pending);
        self.complete_frame_callbacks(frame_callbacks);
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

    fn stack_renderable_descendants_above_parent(&mut self, surface_id: u32) -> bool {
        if !self
            .renderable_surfaces
            .iter()
            .any(|surface| surface.surface_id == surface_id)
        {
            return false;
        }
        let descendant_ids = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .filter(|candidate_id| {
                *candidate_id != surface_id
                    && self.surface_is_descendant_of(*candidate_id, surface_id)
            })
            .collect::<HashSet<_>>();
        if descendant_ids.is_empty() {
            return false;
        }

        let original_order = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        let mut descendants = Vec::new();
        let mut others = Vec::with_capacity(self.renderable_surfaces.len());
        for surface in self.renderable_surfaces.drain(..) {
            if descendant_ids.contains(&surface.surface_id) {
                descendants.push(surface);
            } else {
                others.push(surface);
            }
        }

        let Some(parent_index) = others
            .iter()
            .position(|surface| surface.surface_id == surface_id)
        else {
            self.renderable_surfaces = others;
            self.renderable_surfaces.extend(descendants);
            self.invalidate_surface_origin_cache();
            return true;
        };
        let insert_index = parent_index + 1;
        others.splice(insert_index..insert_index, descendants);
        let changed = others
            .iter()
            .map(|surface| surface.surface_id)
            .ne(original_order);
        self.renderable_surfaces = others;
        if changed {
            self.invalidate_surface_origin_cache();
        }
        changed
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
        self.toplevel_surfaces.insert(
            surface_id,
            ToplevelSurface {
                app_id: None,
                xdg_surface,
                toplevel,
                window: WindowState::default(),
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
        if let Some(buffer) = self.active_dmabuf_buffers.remove(&surface_id) {
            self.queue_dmabuf_buffer_release(buffer);
        }
        let previous_renderable_count = self.renderable_surfaces.len();
        self.renderable_surfaces
            .retain(|surface| surface.surface_id != surface_id);
        if self.renderable_surfaces.len() != previous_renderable_count {
            self.advance_render_generation(RenderGenerationCause::SurfaceUnmap);
        }
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
        let _ = popup_surface.popup.send_event(xdg_popup::Event::Configure {
            x: geometry.x,
            y: geometry.y,
            width: geometry.width,
            height: geometry.height,
        });
        let serial = self.next_configure_serial();
        let _ = popup_surface
            .xdg_surface
            .send_event(xdg_surface::Event::Configure { serial });
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
            let _ = toplevel
                .toplevel
                .send_event(xdg_toplevel::Event::Configure {
                    width: 0,
                    height: 0,
                    states: Vec::new(),
                });
            let serial = self.next_configure_serial();
            let _ = toplevel
                .xdg_surface
                .send_event(xdg_surface::Event::Configure { serial });
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
        self.flush_pending_resize_configure();
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
        let width = width.max(MIN_WINDOW_WIDTH);
        let height = height.max(MIN_WINDOW_HEIGHT);
        if !self.toplevel_surfaces.contains_key(&surface_id) {
            return false;
        };
        let pending = PendingResizeConfigure {
            surface_id,
            width,
            height,
            placement,
            edges,
            resizing: true,
        };
        let duplicate = self.pending_resize_configure == Some(pending);
        if duplicate {
            return false;
        }
        self.pending_resize_configure = Some(pending);
        true
    }

    fn flush_pending_resize_configure(&mut self) -> bool {
        let Some(pending) = self.pending_resize_configure.take() else {
            return false;
        };
        if self
            .pending_resize_commits
            .contains_key(&pending.surface_id)
        {
            self.pending_resize_configure = Some(pending);
            return false;
        }
        self.send_resize_configure_to(
            pending.surface_id,
            pending.width,
            pending.height,
            pending.placement,
            pending.edges,
            pending.resizing,
        )
    }

    fn send_resize_end_configure(&mut self, surface_id: u32, edges: ResizeEdges) -> bool {
        if let Some(pending) = self.pending_resize_configure {
            return self.send_resize_configure_to(
                pending.surface_id,
                pending.width,
                pending.height,
                pending.placement,
                pending.edges,
                false,
            );
        }

        if let Some(resize) = self.pending_resize_commits.get(&surface_id).copied() {
            return self.send_resize_configure_to(
                surface_id,
                resize.width,
                resize.height,
                resize.placement,
                resize.edges,
                false,
            );
        }

        if let Some(resize) = self.latest_sent_resize_commit(surface_id) {
            return self.send_resize_configure_to(
                surface_id,
                resize.width,
                resize.height,
                resize.placement,
                resize.edges,
                false,
            );
        }

        let Some(geometry) = self.current_root_window_geometry(surface_id) else {
            return false;
        };
        self.send_resize_configure_to(
            surface_id,
            geometry.width,
            geometry.height,
            geometry.placement,
            edges,
            false,
        )
    }

    fn pending_resize_configure_is_flushable(&self) -> bool {
        self.pending_resize_configure.is_some_and(|pending| {
            !self
                .pending_resize_commits
                .contains_key(&pending.surface_id)
        })
    }

    fn send_resize_configure_to(
        &mut self,
        surface_id: u32,
        width: u32,
        height: u32,
        placement: SurfacePlacement,
        edges: ResizeEdges,
        resizing: bool,
    ) -> bool {
        let resizing_states = [xdg_toplevel::State::Resizing];
        let states = if resizing {
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
            edges,
            resizing,
        }
        .resize_commit(serial);
        self.sent_resize_commits
            .insert((surface_id, serial), resize);
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: resize configure surface {surface_id} serial={serial} size={}x{} placement={},{} edges={:?} resizing={}",
                resize.width,
                resize.height,
                resize.placement.local_x,
                resize.placement.local_y,
                resize.edges,
                resizing,
            );
        }
        true
    }

    fn latest_sent_resize_commit(&self, surface_id: u32) -> Option<PendingResizeCommit> {
        self.sent_resize_commits
            .iter()
            .filter_map(|((sent_surface_id, _), resize)| {
                (*sent_surface_id == surface_id).then_some(*resize)
            })
            .max_by_key(|resize| resize.serial)
    }

    fn take_pending_resize_commit_placement(
        &mut self,
        surface_id: u32,
        pending: &PendingSurfaceBuffer,
    ) -> io::Result<Option<SurfacePlacement>> {
        let Some(resize) = self.pending_resize_commits.remove(&surface_id) else {
            return Ok(None);
        };
        let buffer_width = pending.data.width()?;
        let buffer_height = pending.data.height()?;
        let committed_size = self
            .xdg_window_geometry_size(surface_id)
            .map(|(width, height)| BufferSize { width, height })
            .or(pending.surface_size)
            .unwrap_or(BufferSize {
                width: buffer_width,
                height: buffer_height,
            });
        if !resize_commit_matches_size(resize, committed_size) {
            self.pending_resize_commits.insert(surface_id, resize);
            return Ok(None);
        }
        let placement =
            resize.placement_for_committed_size(committed_size.width, committed_size.height);
        Ok(Some(placement))
    }

    fn ack_xdg_surface_configure(&mut self, surface_id: u32, serial: u32) {
        let resize = self
            .sent_resize_commits
            .iter()
            .filter_map(|((sent_surface_id, sent_serial), resize)| {
                (*sent_surface_id == surface_id && *sent_serial <= serial).then_some(*resize)
            })
            .max_by_key(|resize| resize.serial);
        self.sent_resize_commits
            .retain(|(sent_surface_id, sent_serial), _| {
                *sent_surface_id != surface_id || *sent_serial > serial
            });
        if let Some(resize) = resize {
            if compositor_debug_surface_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: ack resize surface {surface_id} serial={serial} matched_serial={} size={}x{} placement={},{} edges={:?}",
                    resize.serial,
                    resize.width,
                    resize.height,
                    resize.placement.local_x,
                    resize.placement.local_y,
                    resize.edges,
                );
            }
            self.pending_resize_commits.insert(surface_id, resize);
        }
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
        let width = width.max(MIN_WINDOW_WIDTH);
        let height = height.max(MIN_WINDOW_HEIGHT);
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
        let Some(target) = self.pointer_target_at(self.last_pointer_x, self.last_pointer_y) else {
            self.clear_pointer_focus();
            return;
        };

        self.ensure_pointer_focus(&target.surface);
        self.send_pointer_enter_if_needed(&target);
    }

    fn surface_resource_by_id(&self, surface_id: u32) -> Option<wl_surface::WlSurface> {
        self.surface_resources.get(&surface_id).cloned()
    }

    fn ensure_pointer_focus(&mut self, surface: &wl_surface::WlSurface) {
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

    fn send_pointer_enter_if_needed(&mut self, target: &PointerTarget) {
        self.pointer_resources.retain(Resource::is_alive);
        let pointers = self
            .pointer_resources
            .iter()
            .filter(|pointer| resource_belongs_to_surface_client(*pointer, &target.surface))
            .cloned()
            .collect::<Vec<_>>();

        for pointer in pointers {
            if let Some(index) = self
                .pointer_entered_surfaces
                .iter()
                .position(|(resource, _)| same_wayland_resource(resource, &pointer))
            {
                if same_surface_resource(&self.pointer_entered_surfaces[index].1, &target.surface) {
                    continue;
                }

                let (_, previous_surface) = self.pointer_entered_surfaces.remove(index);
                if resource_belongs_to_surface_client(&pointer, &previous_surface) {
                    let serial = self.next_configure_serial();
                    let _ = pointer.send_event(wl_pointer::Event::Leave {
                        serial,
                        surface: previous_surface,
                    });
                    send_pointer_frame_if_supported(&pointer);
                }
            }

            let serial = self.next_configure_serial();
            let _ = pointer.send_event(wl_pointer::Event::Enter {
                serial,
                surface: target.surface.clone(),
                surface_x: target.surface_x,
                surface_y: target.surface_y,
            });
            self.pointer_entered_surfaces
                .push((pointer, target.surface.clone()));
        }
    }

    fn clear_pointer_focus(&mut self) {
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
            || self
                .pending_explicit_sync_commits
                .iter()
                .any(|commit| !commit.frame_callbacks.is_empty())
            || self
                .surface_resources
                .values()
                .filter_map(Resource::data::<SurfaceData>)
                .any(SurfaceData::has_frame_callbacks)
    }

    fn has_only_pending_surface_frame_callbacks(&self) -> bool {
        self.pending_resize_configure.is_none()
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
            || !self.pending_explicit_sync_commits.is_empty()
            || !self.pending_color_info.is_empty()
    }

    fn has_pending_frame_work(&self) -> bool {
        self.pending_resize_configure_is_flushable()
            || self.has_pending_frame_callbacks()
            || !self.pending_presentation_feedbacks.is_empty()
    }

    fn complete_pending_presentation_feedbacks(&mut self) {
        let feedbacks = std::mem::take(&mut self.pending_presentation_feedbacks);
        if feedbacks.is_empty() {
            return;
        }

        let timestamp = presentation_timestamp();
        let sequence = self.render_generation;
        for pending in feedbacks {
            if !pending.surface.is_alive() {
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
                timestamp.tv_sec_hi,
                timestamp.tv_sec_lo,
                timestamp.tv_nsec,
                self.output_refresh.presentation_refresh_nsec(),
                (sequence >> 32) as u32,
                sequence as u32,
                wp_presentation_feedback::Kind::Vsync | wp_presentation_feedback::Kind::HwClock,
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

    fn commit_ready_explicit_sync_buffers(&mut self) {
        let mut waiting = Vec::new();
        for commit in std::mem::take(&mut self.pending_explicit_sync_commits) {
            if commit.acquire.is_signaled() {
                self.commit_surface_buffer(commit.surface_id, commit.pending, commit.damage);
                self.pending_frame_callbacks.extend(commit.frame_callbacks);
            } else {
                waiting.push(commit);
            }
        }
        self.pending_explicit_sync_commits = waiting;
    }
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
    if pending.data.is_shm()
        && surface.buffer_size() == buffer_size
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
