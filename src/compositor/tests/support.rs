use super::*;
use crate::render_backend::buffer::SurfaceBufferSource;
use crate::render_backend::egl_gles::EglGlesDmabufFormat;
use crate::syncobj::DrmSyncobjTimeline;
use crate::wayland_drm::client::wl_drm as client_wl_drm;
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::{
        fd::{AsFd, FromRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{
        wl_buffer as client_wl_buffer, wl_callback as client_wl_callback,
        wl_compositor as client_wl_compositor, wl_data_device as client_wl_data_device,
        wl_data_device_manager as client_wl_data_device_manager,
        wl_data_offer as client_wl_data_offer,
        wl_data_source as client_wl_data_source, wl_keyboard as client_wl_keyboard,
        wl_output as client_wl_output, wl_pointer as client_wl_pointer,
        wl_region as client_wl_region, wl_registry, wl_seat as client_wl_seat,
        wl_shm as client_wl_shm, wl_shm_pool as client_wl_shm_pool,
        wl_subcompositor as client_wl_subcompositor, wl_subsurface as client_wl_subsurface,
        wl_surface as client_wl_surface,
    },
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1 as client_zwp_linux_buffer_params_v1,
    zwp_linux_dmabuf_feedback_v1 as client_zwp_linux_dmabuf_feedback_v1,
    zwp_linux_dmabuf_v1 as client_zwp_linux_dmabuf_v1,
};
use wayland_protocols::wp::linux_drm_syncobj::v1::client::{
    wp_linux_drm_syncobj_manager_v1 as client_wp_linux_drm_syncobj_manager_v1,
    wp_linux_drm_syncobj_surface_v1 as client_wp_linux_drm_syncobj_surface_v1,
    wp_linux_drm_syncobj_timeline_v1 as client_wp_linux_drm_syncobj_timeline_v1,
};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1 as client_wp_fractional_scale_manager_v1,
    wp_fractional_scale_v1 as client_wp_fractional_scale_v1,
};
use wayland_protocols::wp::idle_inhibit::zv1::client::{
    zwp_idle_inhibit_manager_v1 as client_zwp_idle_inhibit_manager_v1,
    zwp_idle_inhibitor_v1 as client_zwp_idle_inhibitor_v1,
};
use wayland_protocols::wp::pointer_constraints::zv1::client::{
    zwp_confined_pointer_v1 as client_zwp_confined_pointer_v1,
    zwp_locked_pointer_v1 as client_zwp_locked_pointer_v1,
    zwp_pointer_constraints_v1 as client_zwp_pointer_constraints_v1,
};
use wayland_protocols::wp::pointer_warp::v1::client::{
    wp_pointer_warp_v1 as client_wp_pointer_warp_v1,
};
use wayland_protocols::wp::presentation_time::client::{
    wp_presentation as client_wp_presentation,
    wp_presentation_feedback as client_wp_presentation_feedback,
};
use wayland_protocols::wp::relative_pointer::zv1::client::{
    zwp_relative_pointer_manager_v1 as client_zwp_relative_pointer_manager_v1,
    zwp_relative_pointer_v1 as client_zwp_relative_pointer_v1,
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport as client_wp_viewport, wp_viewporter as client_wp_viewporter,
};
use wayland_protocols::xdg::shell::client::{
    xdg_popup as client_xdg_popup, xdg_positioner as client_xdg_positioner,
    xdg_surface as client_xdg_surface, xdg_toplevel as client_xdg_toplevel,
    xdg_wm_base as client_xdg_wm_base,
};

#[derive(Default)]
struct RegistryTestState {
    frame_done: bool,
    frame_done_time: Option<u32>,
    keyboard_key: bool,
    keyboard_key_serial: Option<u32>,
    keyboard_keys: Vec<u32>,
    keyboard_keymap: bool,
    keyboard_keymap_bytes: Vec<u8>,
    keyboard_keymap_size: u32,
    keyboard_mods_depressed: Vec<u32>,
    keyboard_repeat_info: bool,
    pointer_enter: bool,
    pointer_enter_count: usize,
    pointer_leave_count: usize,
    pointer_enter_serial: Option<u32>,
    pointer_enter_serials: Vec<(u32, u32)>,
    pointer_motion: bool,
    pointer_button: bool,
    pointer_button_serial: Option<u32>,
    pointer_enter_surface_id: Option<u32>,
    pointer_button_surface_id: Option<u32>,
    pointer_button_surface_ids: Vec<u32>,
    pointer_axis: bool,
    pointer_vertical_axis: Option<f64>,
    pointer_horizontal_axis: Option<f64>,
    pointer_frame_count: usize,
    pointer_frame_resource_ids: Vec<u32>,
    pointer_enter_frame_count: usize,
    pointer_enter_without_frame_count: usize,
    pointer_event_log: Vec<&'static str>,
    pointer_surface_x: Option<f64>,
    pointer_surface_y: Option<f64>,
    relative_motion_count: usize,
    relative_motion_utime: Option<u64>,
    relative_motion_dx: Option<f64>,
    relative_motion_dy: Option<f64>,
    relative_motion_dx_unaccel: Option<f64>,
    relative_motion_dy_unaccel: Option<f64>,
    relative_motion_resource_ids: Vec<u32>,
    sdl_pending_relative_motion_count: usize,
    sdl_camera_motion_count: usize,
    locked_count: usize,
    unlocked_count: usize,
    confined_count: usize,
    unconfined_count: usize,
    parent_surface_id: Option<u32>,
    child_surface_id: Option<u32>,
    second_child_surface_id: Option<u32>,
    keyboard_enter_surface_id: Option<u32>,
    keyboard_enter_count: usize,
    keyboard_leave_count: usize,
    keyboard_event_log: Vec<&'static str>,
    surface_enter_count: usize,
    seat_has_keyboard: bool,
    output_done: bool,
    output_mode_count: usize,
    output_scale_count: usize,
    output_name: bool,
    output_description: bool,
    output_width: i32,
    output_height: i32,
    output_refresh_millihertz: i32,
    seat_name: bool,
    seat_has_pointer: bool,
    surface_configured: bool,
    surface_configure_count: usize,
    surface_configure_serials: Vec<u32>,
    popup_configured: bool,
    popup_configure_count: usize,
    popup_repositioned_token: Option<u32>,
    popup_x: i32,
    popup_y: i32,
    popup_width: i32,
    popup_height: i32,
    popup_done: bool,
    configured_before_initial_commit: bool,
    configured_after_initial_commit: bool,
    toplevel_configured: bool,
    toplevel_configure_count: usize,
    toplevel_width: i32,
    toplevel_height: i32,
    toplevel_states: Vec<u8>,
    dmabuf_modifier: bool,
    dmabuf_failed: bool,
    dmabuf_created: bool,
    dmabuf_feedback_main_device: bool,
    dmabuf_feedback_format_table: bool,
    dmabuf_feedback_format_table_size: u32,
    dmabuf_feedback_tranche_formats: bool,
    dmabuf_feedback_done: bool,
    wl_drm_device: bool,
    wl_drm_capabilities: bool,
    wl_drm_format: bool,
    wl_drm_authenticated: bool,
    buffer_release_count: usize,
    presentation_presented_count: usize,
    presentation_discarded_count: usize,
    presentation_kind: Option<client_wp_presentation_feedback::Kind>,
    presentation_clock_id: Option<u32>,
    presentation_timestamp: Option<(u32, u32, u32)>,
    presentation_sequence: Option<(u32, u32)>,
    fractional_preferred_scales: Vec<u32>,
    data_device_selection_offer: Option<client_wl_data_offer::WlDataOffer>,
    data_offer_mime_types: Vec<String>,
    data_source_send_mime_types: Vec<String>,
    data_source_cancelled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct XdgRoleSnapshot {
    surface_registered: bool,
    configured: bool,
    toplevel_count: usize,
    toplevel_registered: bool,
    popup_count: usize,
    window_geometry_present: bool,
    placement: Option<SurfacePlacement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderableSurfaceSnapshot {
    surface_id: u32,
    width: u32,
    height: u32,
    parent_surface_id: Option<u32>,
}

impl RegistryTestState {
    fn toplevel_has_state(&self, expected: client_xdg_toplevel::State) -> bool {
        let expected = (expected as u32).to_ne_bytes();
        self.toplevel_states
            .chunks_exact(4)
            .any(|state| state == expected)
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for RegistryTestState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_wl_compositor::WlCompositor, ()> for RegistryTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_wl_compositor::WlCompositor,
        _event: client_wl_compositor::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_wl_surface::WlSurface, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_wl_surface::WlSurface,
        event: client_wl_surface::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let client_wl_surface::Event::Enter { .. } = event {
            state.surface_enter_count += 1;
        }
    }
}

impl Dispatch<client_wl_subcompositor::WlSubcompositor, ()> for RegistryTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_wl_subcompositor::WlSubcompositor,
        _event: client_wl_subcompositor::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_wl_subsurface::WlSubsurface, ()> for RegistryTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_wl_subsurface::WlSubsurface,
        _event: client_wl_subsurface::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_wl_region::WlRegion, ()> for RegistryTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_wl_region::WlRegion,
        _event: client_wl_region::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_wl_data_device_manager::WlDataDeviceManager, ()> for RegistryTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_wl_data_device_manager::WlDataDeviceManager,
        _event: client_wl_data_device_manager::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_wl_data_device::WlDataDevice, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_wl_data_device::WlDataDevice,
        event: client_wl_data_device::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            client_wl_data_device::Event::DataOffer { .. } => {}
            client_wl_data_device::Event::Selection { id } => {
                state.data_device_selection_offer = id;
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(
        RegistryTestState,
        client_wl_data_device::WlDataDevice,
        [0 => (client_wl_data_offer::WlDataOffer, ())]
    );
}

impl Dispatch<client_wl_data_offer::WlDataOffer, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_wl_data_offer::WlDataOffer,
        event: client_wl_data_offer::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let client_wl_data_offer::Event::Offer { mime_type } = event {
            state.data_offer_mime_types.push(mime_type);
        }
    }
}

impl Dispatch<client_wl_data_source::WlDataSource, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_wl_data_source::WlDataSource,
        event: client_wl_data_source::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            client_wl_data_source::Event::Send { mime_type, fd } => {
                state.data_source_send_mime_types.push(mime_type);
                let mut file = File::from(fd);
                let _ = file.write_all(b"clipboard payload");
            }
            client_wl_data_source::Event::Cancelled => {
                state.data_source_cancelled = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<client_wl_callback::WlCallback, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_wl_callback::WlCallback,
        event: client_wl_callback::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let client_wl_callback::Event::Done { callback_data } = event {
            state.frame_done = true;
            state.frame_done_time = Some(callback_data);
        }
    }
}

impl Dispatch<client_wl_output::WlOutput, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_wl_output::WlOutput,
        event: client_wl_output::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            client_wl_output::Event::Done => {
                state.output_done = true;
            }
            client_wl_output::Event::Mode {
                width,
                height,
                refresh,
                ..
            } => {
                state.output_mode_count += 1;
                state.output_width = width;
                state.output_height = height;
                state.output_refresh_millihertz = refresh;
            }
            client_wl_output::Event::Scale { .. } => {
                state.output_scale_count += 1;
            }
            client_wl_output::Event::Name { .. } => {
                state.output_name = true;
            }
            client_wl_output::Event::Description { .. } => {
                state.output_description = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<client_wl_seat::WlSeat, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_wl_seat::WlSeat,
        event: client_wl_seat::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let client_wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        {
            state.seat_has_pointer = capabilities.contains(client_wl_seat::Capability::Pointer);
            state.seat_has_keyboard = capabilities.contains(client_wl_seat::Capability::Keyboard);
        } else if let client_wl_seat::Event::Name { .. } = event {
            state.seat_name = true;
        }
    }
}

impl Dispatch<client_wl_keyboard::WlKeyboard, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_wl_keyboard::WlKeyboard,
        event: client_wl_keyboard::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            client_wl_keyboard::Event::Enter { surface, .. } => {
                state.keyboard_enter_surface_id = Some(surface.id().protocol_id());
                state.keyboard_enter_count += 1;
                state.keyboard_event_log.push("keyboard_enter");
            }
            client_wl_keyboard::Event::Leave { .. } => {
                state.keyboard_leave_count += 1;
                state.keyboard_event_log.push("keyboard_leave");
            }
            client_wl_keyboard::Event::Keymap { fd, size, .. } => {
                state.keyboard_keymap = true;
                state.keyboard_keymap_size = size;
                let mut bytes = vec![0; size as usize];
                let mut file = File::from(fd);
                let _ = file.read_exact(&mut bytes);
                state.keyboard_keymap_bytes = bytes;
            }
            client_wl_keyboard::Event::Key { serial, key, .. } => {
                state.keyboard_key = true;
                state.keyboard_key_serial = Some(serial);
                state.keyboard_keys.push(key);
                state.keyboard_event_log.push("keyboard_key");
            }
            client_wl_keyboard::Event::Modifiers { mods_depressed, .. } => {
                state.keyboard_mods_depressed.push(mods_depressed);
                state.keyboard_event_log.push("keyboard_modifiers");
            }
            client_wl_keyboard::Event::RepeatInfo { .. } => {
                state.keyboard_repeat_info = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<client_wl_pointer::WlPointer, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_wl_pointer::WlPointer,
        event: client_wl_pointer::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            client_wl_pointer::Event::Enter {
                serial, surface, ..
            } => {
                state.pointer_enter = true;
                state.pointer_enter_count += 1;
                state.pointer_enter_serial = Some(serial);
                state
                    .pointer_enter_serials
                    .push((_proxy.id().protocol_id(), serial));
                state.pointer_enter_surface_id = Some(surface.id().protocol_id());
                state.pointer_event_log.push("enter");
            }
            client_wl_pointer::Event::Leave { .. } => {
                state.pointer_leave_count += 1;
                state.pointer_enter_surface_id = None;
                state.pointer_event_log.push("leave");
            }
            client_wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                state.pointer_motion = true;
                state.pointer_surface_x = Some(surface_x);
                state.pointer_surface_y = Some(surface_y);
                state.pointer_event_log.push("motion");
            }
            client_wl_pointer::Event::Button { serial, state: button_state, .. } => {
                state.pointer_button = true;
                state.pointer_button_serial = Some(serial);
                state.pointer_button_surface_id = state.pointer_enter_surface_id;
                if let Some(surface_id) = state.pointer_enter_surface_id {
                    state.pointer_button_surface_ids.push(surface_id);
                }
                match button_state {
                    WEnum::Value(client_wl_pointer::ButtonState::Pressed) => {
                        state.pointer_event_log.push("button_pressed");
                    }
                    WEnum::Value(client_wl_pointer::ButtonState::Released) => {
                        state.pointer_event_log.push("button_released");
                    }
                    _ => state.pointer_event_log.push("button"),
                }
            }
            client_wl_pointer::Event::Axis {
                axis: WEnum::Value(axis),
                value,
                ..
            } => {
                state.pointer_axis = true;
                match axis {
                    client_wl_pointer::Axis::VerticalScroll => {
                        state.pointer_vertical_axis = Some(value);
                    }
                    client_wl_pointer::Axis::HorizontalScroll => {
                        state.pointer_horizontal_axis = Some(value);
                    }
                    _ => {}
                }
                state.pointer_event_log.push("axis");
            }
            client_wl_pointer::Event::Frame => {
                state.pointer_frame_count += 1;
                state
                    .pointer_frame_resource_ids
                    .push(_proxy.id().protocol_id());
                if state.pointer_event_log.last() == Some(&"enter") {
                    state.pointer_enter_frame_count += 1;
                }
                if state.sdl_pending_relative_motion_count > 0 {
                    state.sdl_camera_motion_count += state.sdl_pending_relative_motion_count;
                    state.sdl_pending_relative_motion_count = 0;
                }
                state.pointer_event_log.push("frame");
                if state.pointer_event_log.contains(&"enter")
                    && !state
                        .pointer_event_log
                        .windows(2)
                        .any(|events| events == ["enter", "frame"])
                {
                    state.pointer_enter_without_frame_count += 1;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1, ()>
    for RegistryTestState
{
    fn event(
        _state: &mut Self,
        _proxy: &client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
        _event: client_zwp_relative_pointer_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_zwp_relative_pointer_v1::ZwpRelativePointerV1, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_zwp_relative_pointer_v1::ZwpRelativePointerV1,
        event: client_zwp_relative_pointer_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let client_zwp_relative_pointer_v1::Event::RelativeMotion {
            utime_hi,
            utime_lo,
            dx,
            dy,
            dx_unaccel,
            dy_unaccel,
        } = event
        {
            state.relative_motion_count += 1;
            state.relative_motion_resource_ids.push(_proxy.id().protocol_id());
            state.relative_motion_utime =
                Some((u64::from(utime_hi) << 32) | u64::from(utime_lo));
            state.relative_motion_dx = Some(dx);
            state.relative_motion_dy = Some(dy);
            state.relative_motion_dx_unaccel = Some(dx_unaccel);
            state.relative_motion_dy_unaccel = Some(dy_unaccel);
            state.sdl_pending_relative_motion_count += 1;
            state.pointer_event_log.push("relative");
        }
    }
}

impl Dispatch<client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1, ()>
    for RegistryTestState
{
    fn event(
        _state: &mut Self,
        _proxy: &client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1,
        _event: client_zwp_pointer_constraints_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_zwp_locked_pointer_v1::ZwpLockedPointerV1, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_zwp_locked_pointer_v1::ZwpLockedPointerV1,
        event: client_zwp_locked_pointer_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            client_zwp_locked_pointer_v1::Event::Locked => {
                state.locked_count += 1;
                state.pointer_event_log.push("locked");
            }
            client_zwp_locked_pointer_v1::Event::Unlocked => {
                state.unlocked_count += 1;
                state.pointer_event_log.push("unlocked");
            }
            _ => {}
        }
    }
}

impl Dispatch<client_zwp_confined_pointer_v1::ZwpConfinedPointerV1, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_zwp_confined_pointer_v1::ZwpConfinedPointerV1,
        event: client_zwp_confined_pointer_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            client_zwp_confined_pointer_v1::Event::Confined => state.confined_count += 1,
            client_zwp_confined_pointer_v1::Event::Unconfined => state.unconfined_count += 1,
            _ => {}
        }
    }
}

impl Dispatch<client_wp_pointer_warp_v1::WpPointerWarpV1, ()> for RegistryTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_wp_pointer_warp_v1::WpPointerWarpV1,
        _event: client_wp_pointer_warp_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1, ()>
    for RegistryTestState
{
    fn event(
        _state: &mut Self,
        _proxy: &client_zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1,
        _event: client_zwp_idle_inhibit_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1, ()> for RegistryTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1,
        _event: client_zwp_idle_inhibitor_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_wl_shm::WlShm, ()> for RegistryTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_wl_shm::WlShm,
        _event: client_wl_shm::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_wl_shm_pool::WlShmPool, ()> for RegistryTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_wl_shm_pool::WlShmPool,
        _event: client_wl_shm_pool::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_wl_buffer::WlBuffer, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_wl_buffer::WlBuffer,
        event: client_wl_buffer::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let client_wl_buffer::Event::Release = event {
            state.buffer_release_count += 1;
        }
    }
}

impl Dispatch<client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
        event: client_zwp_linux_dmabuf_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            client_zwp_linux_dmabuf_v1::Event::Modifier {
                format,
                modifier_hi,
                modifier_lo,
            } => {
                let modifier = ((modifier_hi as u64) << 32) | u64::from(modifier_lo);
                state.dmabuf_modifier |=
                    format == DRM_FORMAT_ARGB8888 && modifier == DRM_FORMAT_MOD_LINEAR;
            }
            client_zwp_linux_dmabuf_v1::Event::Format { format } => {
                state.dmabuf_modifier |= format == DRM_FORMAT_ARGB8888;
            }
            _ => {}
        }
    }
}

impl Dispatch<client_zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        event: client_zwp_linux_buffer_params_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            client_zwp_linux_buffer_params_v1::Event::Failed => {
                state.dmabuf_failed = true;
            }
            client_zwp_linux_buffer_params_v1::Event::Created { .. } => {
                state.dmabuf_created = true;
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(
        RegistryTestState,
        client_zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        [
            0 => (client_wl_buffer::WlBuffer, ())
        ]
    );
}

impl Dispatch<client_wl_drm::WlDrm, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_wl_drm::WlDrm,
        event: client_wl_drm::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            client_wl_drm::Event::Device { name } => {
                state.wl_drm_device |= name.starts_with("/dev/dri/");
            }
            client_wl_drm::Event::Capabilities { value } => {
                state.wl_drm_capabilities |= value & 1 == 1;
            }
            client_wl_drm::Event::Format { format } => {
                state.wl_drm_format |= format == DRM_FORMAT_ARGB8888;
            }
            client_wl_drm::Event::Authenticated => {
                state.wl_drm_authenticated = true;
            }
        }
    }
}

impl Dispatch<client_zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1, ()>
    for RegistryTestState
{
    fn event(
        state: &mut Self,
        _proxy: &client_zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
        event: client_zwp_linux_dmabuf_feedback_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            client_zwp_linux_dmabuf_feedback_v1::Event::MainDevice { device } => {
                state.dmabuf_feedback_main_device |= !device.is_empty();
            }
            client_zwp_linux_dmabuf_feedback_v1::Event::FormatTable { fd: _, size } => {
                state.dmabuf_feedback_format_table |= size >= 16;
                state.dmabuf_feedback_format_table_size = size;
            }
            client_zwp_linux_dmabuf_feedback_v1::Event::TrancheFormats { indices } => {
                state.dmabuf_feedback_tranche_formats |= indices.len() >= 2;
            }
            client_zwp_linux_dmabuf_feedback_v1::Event::Done => {
                state.dmabuf_feedback_done = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1, ()>
    for RegistryTestState
{
    fn event(
        _state: &mut Self,
        _proxy: &client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1,
        _event: client_wp_linux_drm_syncobj_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1, ()>
    for RegistryTestState
{
    fn event(
        _state: &mut Self,
        _proxy: &client_wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1,
        _event: client_wp_linux_drm_syncobj_timeline_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1, ()>
    for RegistryTestState
{
    fn event(
        _state: &mut Self,
        _proxy: &client_wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1,
        _event: client_wp_linux_drm_syncobj_surface_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_wp_presentation::WpPresentation, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_wp_presentation::WpPresentation,
        event: client_wp_presentation::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let client_wp_presentation::Event::ClockId { clk_id } = event {
            state.presentation_clock_id = Some(clk_id);
        }
    }
}

impl Dispatch<client_wp_presentation_feedback::WpPresentationFeedback, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_wp_presentation_feedback::WpPresentationFeedback,
        event: client_wp_presentation_feedback::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            client_wp_presentation_feedback::Event::Presented {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                seq_hi,
                seq_lo,
                flags,
                ..
            } => {
                state.presentation_presented_count += 1;
                state.presentation_timestamp = Some((tv_sec_hi, tv_sec_lo, tv_nsec));
                state.presentation_sequence = Some((seq_hi, seq_lo));
                if let WEnum::Value(flags) = flags {
                    state.presentation_kind = Some(flags);
                }
            }
            client_wp_presentation_feedback::Event::Discarded => {
                state.presentation_discarded_count += 1;
            }
            _ => {}
        }
    }
}

impl Dispatch<client_wp_viewporter::WpViewporter, ()> for RegistryTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_wp_viewporter::WpViewporter,
        _event: client_wp_viewporter::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_wp_viewport::WpViewport, ()> for RegistryTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_wp_viewport::WpViewport,
        _event: client_wp_viewport::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, ()>
    for RegistryTestState
{
    fn event(
        _state: &mut Self,
        _proxy: &client_wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        _event: client_wp_fractional_scale_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_wp_fractional_scale_v1::WpFractionalScaleV1, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_wp_fractional_scale_v1::WpFractionalScaleV1,
        event: client_wp_fractional_scale_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let client_wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            state.fractional_preferred_scales.push(scale);
        }
    }
}

impl Dispatch<client_xdg_wm_base::XdgWmBase, ()> for RegistryTestState {
    fn event(
        _state: &mut Self,
        proxy: &client_xdg_wm_base::XdgWmBase,
        event: client_xdg_wm_base::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let client_xdg_wm_base::Event::Ping { serial } = event {
            proxy.pong(serial);
        }
    }
}

impl Dispatch<client_xdg_surface::XdgSurface, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        proxy: &client_xdg_surface::XdgSurface,
        event: client_xdg_surface::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let client_xdg_surface::Event::Configure { serial } = event {
            state.surface_configured = true;
            state.surface_configure_count += 1;
            state.surface_configure_serials.push(serial);
            proxy.ack_configure(serial);
        }
    }
}

impl Dispatch<client_xdg_toplevel::XdgToplevel, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_xdg_toplevel::XdgToplevel,
        event: client_xdg_toplevel::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let client_xdg_toplevel::Event::Configure {
            width,
            height,
            states,
        } = event
        {
            state.toplevel_configured = true;
            state.toplevel_configure_count += 1;
            state.toplevel_width = width;
            state.toplevel_height = height;
            state.toplevel_states = states;
        }
    }
}

impl Dispatch<client_xdg_positioner::XdgPositioner, ()> for RegistryTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_xdg_positioner::XdgPositioner,
        _event: client_xdg_positioner::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_xdg_popup::XdgPopup, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_xdg_popup::XdgPopup,
        event: client_xdg_popup::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            client_xdg_popup::Event::Configure {
                x,
                y,
                width,
                height,
            } => {
                state.popup_configured = true;
                state.popup_configure_count += 1;
                state.popup_x = x;
                state.popup_y = y;
                state.popup_width = width;
                state.popup_height = height;
            }
            client_xdg_popup::Event::Repositioned { token } => {
                state.popup_repositioned_token = Some(token);
            }
            client_xdg_popup::Event::PopupDone => {
                state.popup_done = true;
            }
            _ => {}
        }
    }
}

fn read_registry_globals(socket_path: &PathBuf) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, _queue) = registry_queue_init::<RegistryTestState>(&connection)?;

    Ok(globals
        .contents()
        .clone_list()
        .into_iter()
        .map(|global| global.interface)
        .collect())
}

fn request_presentation_feedback(
    socket_path: &PathBuf,
) -> Result<Connection, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let presentation: client_wp_presentation::WpPresentation = globals.bind(&qh, 1..=2, ())?;
    let surface = compositor.create_surface(&qh, ());
    let _feedback = presentation.feedback(&surface, &qh, ());
    connection.flush()?;
    Ok(connection)
}

fn create_surface_with_presentation_feedback_and_present(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    completion: ServerCommand,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let presentation: client_wp_presentation::WpPresentation = globals.bind(&qh, 1..=2, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    let _feedback = presentation.feedback(&surface, &qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(completion)?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_surface_with_unpresented_presentation_feedback(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let presentation: client_wp_presentation::WpPresentation = globals.bind(&qh, 1..=2, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    let _feedback = presentation.feedback(&surface, &qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    Ok(())
}

fn create_client_toplevel(socket_path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());

    toplevel.set_app_id("oblivion.test".to_string());
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_configured_client_toplevel(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    toplevel.set_app_id("oblivion.configure-test".to_string());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_toplevel_and_check_initial_commit_configure_order(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    toplevel.set_app_id("oblivion.initial-configure-order".to_string());
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    state.configured_before_initial_commit = state.surface_configured || state.toplevel_configured;

    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    state.configured_after_initial_commit = state.surface_configured && state.toplevel_configured;
    Ok(state)
}

fn recreate_toplevel_role_on_same_surface(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(RegistryTestState, u32, XdgRoleSnapshot), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let surface = compositor.create_surface(&qh, ());
    let surface_id = surface.id().protocol_id();

    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    xdg_surface.set_window_geometry(5, 6, 111, 77);
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    toplevel.set_app_id("oblivion.recreate-a".to_string());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    assert_eq!(state.surface_configure_count, 1);
    assert_eq!(state.toplevel_configure_count, 1);

    toplevel.destroy();
    xdg_surface.destroy();
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    state.surface_configured = false;
    state.toplevel_configured = false;
    state.toplevel_configure_count = 0;

    let recreated_xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let recreated_toplevel = recreated_xdg_surface.get_toplevel(&qh, ());
    recreated_toplevel.set_app_id("oblivion.recreate-b".to_string());
    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    Ok((
        state,
        surface_id,
        capture_xdg_role_snapshot(commands, surface_id),
    ))
}

fn create_popup_and_check_initial_commit_configure_order(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let parent_toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    parent_toplevel.set_app_id("oblivion.popup-initial-configure-parent".to_string());
    parent.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commit_test_buffered_surface(&parent, &shm, &qh, 120, 90)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    state.surface_configured = false;
    state.popup_configured = false;
    state.toplevel_configured = false;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(60, 40);
    positioner.set_anchor_rect(10, 20, 30, 10);
    positioner.set_anchor(client_xdg_positioner::Anchor::BottomRight);
    positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    let _popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    state.configured_before_initial_commit = state.surface_configured || state.popup_configured;

    popup_surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    state.configured_after_initial_commit = state.surface_configured && state.popup_configured;
    Ok(state)
}

fn create_client_toplevel_with_configured_popup(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let parent_toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    parent_toplevel.set_app_id("oblivion.popup-parent".to_string());
    commit_test_buffered_surface(&parent, &shm, &qh, 120, 90)?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(60, 40);
    positioner.set_anchor_rect(10, 20, 30, 10);
    positioner.set_anchor(client_xdg_positioner::Anchor::BottomRight);
    positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    positioner.set_offset(3, 4);
    let _popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 60, 40)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_client_popup_with_constrained_positioner(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let parent_toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    parent_toplevel.set_app_id("oblivion.popup-constraint-parent".to_string());
    commit_test_buffered_surface(&parent, &shm, &qh, 120, 90)?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(80, 50);
    positioner.set_parent_size(120, 90);
    positioner.set_anchor_rect(110, 80, 10, 10);
    positioner.set_anchor(client_xdg_positioner::Anchor::BottomRight);
    positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    positioner.set_constraint_adjustment(
        client_xdg_positioner::ConstraintAdjustment::SlideX
            | client_xdg_positioner::ConstraintAdjustment::SlideY,
    );
    let _popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 80, 50)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_client_popup_then_reposition(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let parent_toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    parent_toplevel.set_app_id("oblivion.popup-reposition-parent".to_string());
    commit_test_buffered_surface(&parent, &shm, &qh, 120, 90)?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let initial_positioner = wm_base.create_positioner(&qh, ());
    initial_positioner.set_size(60, 40);
    initial_positioner.set_anchor_rect(10, 20, 30, 10);
    initial_positioner.set_anchor(client_xdg_positioner::Anchor::BottomRight);
    initial_positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    let popup =
        popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &initial_positioner, &qh, ());
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 60, 40)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    let repositioner = wm_base.create_positioner(&qh, ());
    repositioner.set_size(50, 30);
    repositioner.set_anchor_rect(5, 7, 1, 1);
    repositioner.set_anchor(client_xdg_positioner::Anchor::TopLeft);
    repositioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    repositioner.set_offset(1, 1);
    popup.reposition(&repositioner, 77);
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 50, 30)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_client_popup_with_window_geometry(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let parent_toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    parent_toplevel.set_app_id("oblivion.popup-window-geometry-parent".to_string());
    parent_xdg_surface.set_window_geometry(8, 9, 100, 80);
    commit_test_buffered_surface(&parent, &shm, &qh, 120, 90)?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    popup_xdg_surface.set_window_geometry(2, 3, 40, 30);
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(40, 30);
    positioner.set_anchor_rect(10, 20, 1, 1);
    positioner.set_anchor(client_xdg_positioner::Anchor::TopLeft);
    positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    let _popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 40, 30)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_non_reactive_popup_then_set_window_geometry(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let parent_xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let parent_toplevel = parent_xdg_surface.get_toplevel(&qh, ());
    parent_toplevel.set_app_id("oblivion.popup-gecko-parent".to_string());
    commit_test_buffered_surface(&parent, &shm, &qh, 120, 90)?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(1, 1);
    positioner.set_anchor_rect(0, 0, 1, 1);
    let _popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    popup_surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    assert_eq!(state.popup_configure_count, 1);

    state.popup_configure_count = 0;
    popup_xdg_surface.set_window_geometry(0, 0, 177, 493);
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 177, 493)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_grabbed_popup_then_release_under_cursor(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(RegistryTestState, u32), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let _pointer = seat.get_pointer(&qh, ());
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let (_parent_surface, parent_xdg_surface, parent_toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 120, 90)?;
    parent_toplevel.set_app_id("oblivion.popup-grab-parent".to_string());
    _parent_surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0 + 47),
        y: f64::from(render::FIRST_SURFACE_OFFSET.1 + 37),
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x111,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let serial = state
        .pointer_button_serial
        .ok_or_else(|| io::Error::other("pointer button serial was not delivered"))?;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_surface_id = popup_surface.id().protocol_id();
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(60, 40);
    positioner.set_anchor_rect(10, 20, 30, 10);
    positioner.set_anchor(client_xdg_positioner::Anchor::BottomRight);
    positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    positioner.set_offset(3, 4);
    let popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    popup.grab(&seat, serial);
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 60, 40)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    state.pointer_button_surface_id = None;
    commands.send(ServerCommand::PointerButton {
        button: 0x111,
        pressed: false,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok((state, popup_surface_id))
}

fn create_grabbed_popup_under_cursor(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(RegistryTestState, u32), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let _pointer = seat.get_pointer(&qh, ());
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let (_parent_surface, parent_xdg_surface, parent_toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 120, 90)?;
    parent_toplevel.set_app_id("oblivion.popup-grab-parent".to_string());
    _parent_surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0 + 47),
        y: f64::from(render::FIRST_SURFACE_OFFSET.1 + 37),
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x111,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let serial = state
        .pointer_button_serial
        .ok_or_else(|| io::Error::other("pointer button serial was not delivered"))?;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_surface_id = popup_surface.id().protocol_id();
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(60, 40);
    positioner.set_anchor_rect(10, 20, 30, 10);
    positioner.set_anchor(client_xdg_positioner::Anchor::BottomRight);
    positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    positioner.set_offset(3, 4);
    let popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    popup.grab(&seat, serial);
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 60, 40)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    Ok((state, popup_surface_id))
}

fn create_grabbed_popup_then_click_outside(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let _pointer = seat.get_pointer(&qh, ());
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let (parent_surface, parent_xdg_surface, parent_toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 120, 90)?;
    parent_toplevel.set_app_id("oblivion.popup-grab-dismiss-parent".to_string());
    parent_surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0 + 47),
        y: f64::from(render::FIRST_SURFACE_OFFSET.1 + 37),
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x111,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let serial = state
        .pointer_button_serial
        .ok_or_else(|| io::Error::other("pointer button serial was not delivered"))?;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(60, 40);
    positioner.set_anchor_rect(10, 20, 30, 10);
    positioner.set_anchor(client_xdg_positioner::Anchor::BottomRight);
    positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    positioner.set_offset(3, 4);
    let popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    popup.grab(&seat, serial);
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 60, 40)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::PointerButton {
        button: 0x111,
        pressed: false,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    state.popup_done = false;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0 + 5),
        y: f64::from(render::FIRST_SURFACE_OFFSET.1 + 5),
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok(state)
}

fn create_grabbed_popup_then_axis_outside(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let _pointer = seat.get_pointer(&qh, ());
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let (parent_surface, parent_xdg_surface, parent_toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 120, 90)?;
    parent_toplevel.set_app_id("oblivion.popup-grab-axis-parent".to_string());
    parent_surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0 + 47),
        y: f64::from(render::FIRST_SURFACE_OFFSET.1 + 37),
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x111,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let serial = state
        .pointer_button_serial
        .ok_or_else(|| io::Error::other("pointer button serial was not delivered"))?;

    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg_surface = wm_base.get_xdg_surface(&popup_surface, &qh, ());
    let positioner = wm_base.create_positioner(&qh, ());
    positioner.set_size(60, 40);
    positioner.set_anchor_rect(10, 20, 30, 10);
    positioner.set_anchor(client_xdg_positioner::Anchor::BottomRight);
    positioner.set_gravity(client_xdg_positioner::Gravity::BottomRight);
    positioner.set_offset(3, 4);
    let popup = popup_xdg_surface.get_popup(Some(&parent_xdg_surface), &positioner, &qh, ());
    popup.grab(&seat, serial);
    commit_test_buffered_surface(&popup_surface, &shm, &qh, 60, 40)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;

    state.pointer_axis = false;
    state.pointer_vertical_axis = None;
    commands.send(ServerCommand::PointerButton {
        button: 0x111,
        pressed: false,
    })?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0 + 5),
        y: f64::from(render::FIRST_SURFACE_OFFSET.1 + 5),
    })?;
    commands.send(ServerCommand::PointerAxis {
        horizontal: 0.0,
        vertical: 15.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok(state)
}

fn create_client_surface_and_wait_for_enter(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let _output: client_wl_output::WlOutput = globals.bind(&qh, 1..=4, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
    commit_test_buffered_surface(&surface, &shm, &qh, 40, 30)?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_idle_inhibitor_for_surface_and_capture_state(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let idle_manager: client_zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
    let _inhibitor = idle_manager.create_inhibitor(&surface, &qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    wait_for_server_commands(commands);
    let (reply, rx) = mpsc::channel();
    commands.send(ServerCommand::CaptureIdleInhibited(reply))?;
    Ok(rx.recv_timeout(Duration::from_secs(1))?)
}

fn create_client_surface_with_viewport_destination(
    socket_path: &PathBuf,
    buffer_width: u32,
    buffer_height: u32,
    destination_width: u32,
    destination_height: u32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let viewporter: client_wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
    let viewport = viewporter.get_viewport(&surface, &qh, ());
    viewport.set_destination(destination_width as i32, destination_height as i32);
    commit_test_buffered_surface(
        &surface,
        &shm,
        &qh,
        buffer_width as usize,
        buffer_height as usize,
    )?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_client_surface_with_buffer_offset(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
    attach_test_buffered_surface(&surface, &shm, &qh, 40, 30)?;
    surface.offset(5, 7);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(())
}

fn create_configured_client_toplevel_then_resize_focused(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    width: u32,
    height: u32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    toplevel.set_app_id("oblivion.resize-test".to_string());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::ResizeFocusedTo { width, height })?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_buffered_toplevel_then_resize_drag(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let width = 100;
    let height = 80;
    let pixels = vec![0xff20_3040; width * height];
    let file = create_test_shm_file(&pixels)?;
    let pool = shm.create_pool(file.as_fd(), (pixels.len() * 4) as i32, &qh, ());
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        (width * 4) as i32,
        client_wl_shm::Format::Argb8888,
        &qh,
        (),
    );

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, width as i32, height as i32);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginResize {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 90.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 70.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 290.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 190.0,
    })?;
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_buffered_toplevel_then_toggle_maximize(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    create_buffered_toplevel_then_window_commands(
        socket_path,
        commands,
        &[ServerCommand::ToggleMaximizeFocused],
    )
}

fn create_buffered_toplevel_then_toggle_maximize_twice(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    create_buffered_toplevel_then_window_commands(
        socket_path,
        commands,
        &[
            ServerCommand::ToggleMaximizeFocused,
            ServerCommand::ToggleMaximizeFocused,
        ],
    )
}

fn create_buffered_toplevel_then_toggle_fullscreen(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    create_buffered_toplevel_then_window_commands(
        socket_path,
        commands,
        &[ServerCommand::ToggleFullscreenFocused],
    )
}

fn create_buffered_toplevel_then_window_commands(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    window_commands: &[ServerCommand],
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    for command in window_commands {
        commands.send(command.clone())?;
        wait_for_server_commands(commands);
        queue.roundtrip(&mut state)?;
    }
    Ok(state)
}

fn create_buffered_toplevel_then_coalesced_resize_drag(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 314.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 214.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 330.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 224.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_buffered_toplevel_then_active_resize_configure(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    })?;
    commands.send(ServerCommand::PrepareFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_buffered_toplevel_then_resize_drag_without_client_commit_between_frames(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    })?;
    commands.send(ServerCommand::PrepareFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 384.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 264.0,
    })?;
    commands.send(ServerCommand::PrepareFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_buffered_toplevel_then_queue_resize_configure_and_capture_pending_frame_work(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    })?;
    wait_for_server_commands(commands);
    let pending = capture_pending_frame_work(commands);
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    Ok(pending)
}

fn create_buffered_toplevel_then_queue_resize_configure_and_unmap(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    })?;
    wait_for_server_commands(commands);
    surface.attach(None, 0, 0);
    surface.commit();
    connection.flush()?;
    wait_for_server_commands(commands);
    let pending = capture_pending_frame_work(commands);
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    Ok(pending)
}

fn create_buffered_toplevel_then_prepare_queued_resize_configure_and_capture_pending_frame_work(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(bool, bool), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    })?;
    wait_for_server_commands(commands);
    let before_prepare = capture_pending_frame_work(commands);
    commands.send(ServerCommand::PrepareFrame)?;
    wait_for_server_commands(commands);
    let after_prepare = capture_pending_frame_work(commands);
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    Ok((before_prepare, after_prepare))
}

fn create_buffered_toplevel_then_resize_drag_and_release(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let resized_width = usize::try_from(state.toplevel_width)?;
    let resized_height = usize::try_from(state.toplevel_height)?;
    commit_test_buffered_surface(&surface, &shm, &qh, resized_width, resized_height)?;
    connection.flush()?;
    wait_for_server_commands(commands);

    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_buffered_toplevel_then_alt_top_left_resize_drag_and_release(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginResize {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 40.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 40.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0),
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 10.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    commit_test_buffered_surface(
        &surface,
        &shm,
        &qh,
        usize::try_from(state.toplevel_width)?,
        usize::try_from(state.toplevel_height)?,
    )?;
    connection.flush()?;
    wait_for_server_commands(commands);

    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_csd_toplevel_then_resize_drag_commit_buffer_margin_and_release(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 332, 242)?;
    xdg_surface.set_window_geometry(16, 10, 300, 200);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginResize {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 200.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 120.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 240.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 150.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    xdg_surface.set_window_geometry(16, 10, state.toplevel_width, state.toplevel_height);
    commit_test_buffered_surface(
        &surface,
        &shm,
        &qh,
        usize::try_from(state.toplevel_width + 32)?,
        usize::try_from(state.toplevel_height + 42)?,
    )?;
    connection.flush()?;
    wait_for_server_commands(commands);

    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_buffered_toplevel_then_measure_configure_only_resize_generation(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(u64, u64), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    let before_resize = capture_render_generation(commands);

    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    let after_resize = capture_render_generation(commands);

    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    Ok((before_resize, after_resize))
}

fn create_buffered_toplevel_request_move_and_drag(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let _pointer = seat.get_pointer(&qh, ());
    let (surface, _xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 100, 80)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 12.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    })?;
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: true,
    })?;
    queue.roundtrip(&mut state)?;

    let serial = state
        .pointer_button_serial
        .ok_or_else(|| io::Error::other("pointer button serial was not delivered"))?;
    toplevel._move(&seat, serial);
    connection.flush()?;
    wait_for_server_commands(commands);
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 52.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 42.0,
    })?;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: false,
    })?;
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    Ok(state)
}

fn create_toplevel_request_move_from_client_chrome_surface(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let _pointer = seat.get_pointer(&qh, ());

    let (surface, _xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 100, 80)?;
    surface.commit();

    let chrome_width = 120;
    let chrome_height = 20;
    let chrome_pixels = vec![0xff70_7070; chrome_width * chrome_height];
    let chrome_file = create_test_shm_file(&chrome_pixels)?;
    let chrome_pool = shm.create_pool(
        chrome_file.as_fd(),
        (chrome_pixels.len() * 4) as i32,
        &qh,
        (),
    );
    let chrome_buffer = chrome_pool.create_buffer(
        0,
        chrome_width as i32,
        chrome_height as i32,
        (chrome_width * 4) as i32,
        client_wl_shm::Format::Argb8888,
        &qh,
        (),
    );
    let chrome = compositor.create_surface(&qh, ());
    chrome.attach(Some(&chrome_buffer), 0, 0);
    chrome.damage_buffer(0, 0, chrome_width as i32, chrome_height as i32);
    chrome.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0 + render::SURFACE_CASCADE_STEP) + 12.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1 + render::SURFACE_CASCADE_STEP) + 14.0,
    })?;
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: true,
    })?;
    queue.roundtrip(&mut state)?;

    let serial = state
        .pointer_button_serial
        .ok_or_else(|| io::Error::other("pointer button serial was not delivered"))?;
    toplevel._move(&seat, serial);
    connection.flush()?;
    wait_for_server_commands(commands);
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0 + render::SURFACE_CASCADE_STEP) + 92.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1 + render::SURFACE_CASCADE_STEP) + 74.0,
    })?;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: false,
    })?;
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    Ok(state)
}

fn create_buffered_toplevel_request_top_left_resize_and_drag(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let _pointer = seat.get_pointer(&qh, ());
    let (surface, _xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 2.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 2.0,
    })?;
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: true,
    })?;
    queue.roundtrip(&mut state)?;

    let serial = state
        .pointer_button_serial
        .ok_or_else(|| io::Error::other("pointer button serial was not delivered"))?;
    toplevel.resize(&seat, serial, client_xdg_toplevel::ResizeEdge::TopLeft);
    connection.flush()?;
    wait_for_server_commands(commands);
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) - 38.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) - 28.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let resized_width = usize::try_from(state.toplevel_width)?;
    let resized_height = usize::try_from(state.toplevel_height)?;
    commit_test_buffered_surface(&surface, &shm, &qh, resized_width, resized_height)?;
    connection.flush()?;
    wait_for_server_commands(commands);
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: false,
    })?;
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    Ok(state)
}

fn create_buffered_toplevel_then_frame_corner_resize_drag(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    Ok(state)
}

fn create_buffered_toplevel_then_frame_corner_resize_click_with_tiny_motion(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 305.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 205.0,
    })?;
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_buffered_toplevel_then_left_edge_shrink_before_client_commit(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 300, 200)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) - 3.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 100.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 37.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 100.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn bind_output_and_seat(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let _output: client_wl_output::WlOutput = globals.bind(&qh, 1..=4, ())?;
    let _seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn bind_output_at_version(
    socket_path: &PathBuf,
    version: u32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let _output: client_wl_output::WlOutput = globals.bind(&qh, version..=version, ())?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn bind_seat_at_version(
    socket_path: &PathBuf,
    version: u32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let _seat: client_wl_seat::WlSeat = globals.bind(&qh, version..=version, ())?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn bind_output_then_set_output_size(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    width: u32,
    height: u32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let _output: client_wl_output::WlOutput = globals.bind(&qh, 1..=4, ())?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::SetOutputSize { width, height })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn bind_output_then_set_output_refresh(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    refresh_hz: u32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let _output: client_wl_output::WlOutput = globals.bind(&qh, 1..=4, ())?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::SetOutputRefresh { refresh_hz })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn bind_output_then_set_output_refresh_and_size(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    refresh_hz: u32,
    width: u32,
    height: u32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let _output: client_wl_output::WlOutput = globals.bind(&qh, 1..=4, ())?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::SetOutputRefresh { refresh_hz })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::SetOutputSize { width, height })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_fractional_scale_surface_then_set_output_scale(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    scale_factor: f64,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let fractional_scale_manager: client_wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
    let _fractional_scale = fractional_scale_manager.get_fractional_scale(&surface, &qh, ());
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::SetOutputScale { scale_factor })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_duplicate_fractional_scale_surface(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let fractional_scale_manager: client_wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
    let _first_fractional_scale = fractional_scale_manager.get_fractional_scale(&surface, &qh, ());
    let _second_fractional_scale = fractional_scale_manager.get_fractional_scale(&surface, &qh, ());
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_client_surface_with_buffer_scale(
    socket_path: &PathBuf,
    buffer_width: u32,
    buffer_height: u32,
    buffer_scale: i32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
    surface.set_buffer_scale(buffer_scale);
    commit_test_buffered_surface(
        &surface,
        &shm,
        &qh,
        buffer_width as usize,
        buffer_height as usize,
    )?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn request_keyboard_from_seat(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    request_keyboard_from_seat_at_version(socket_path, 7)
}

fn request_keyboard_from_seat_at_version(
    socket_path: &PathBuf,
    version: u32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let seat: client_wl_seat::WlSeat = globals.bind(&qh, version..=version, ())?;
    let _keyboard = seat.get_keyboard(&qh, ());
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_client_subsurface(socket_path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let parent = compositor.create_surface(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());

    subsurface.set_position(10, 12);
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_client_data_device(socket_path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ())?;
    let _source = manager.create_data_source(&qh, ());
    let _device = manager.get_data_device(&seat, &qh, ());
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn forward_clipboard_between_two_clients(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(RegistryTestState, RegistryTestState, String), Box<dyn std::error::Error>> {
    let source_stream = UnixStream::connect(socket_path)?;
    let source_connection = Connection::from_socket(source_stream)?;
    let (source_globals, mut source_queue) =
        registry_queue_init::<RegistryTestState>(&source_connection)?;
    let source_qh = source_queue.handle();

    let source_compositor: client_wl_compositor::WlCompositor =
        source_globals.bind(&source_qh, 1..=6, ())?;
    let source_wm_base: client_xdg_wm_base::XdgWmBase =
        source_globals.bind(&source_qh, 1..=6, ())?;
    let source_seat: client_wl_seat::WlSeat = source_globals.bind(&source_qh, 1..=7, ())?;
    let source_manager: client_wl_data_device_manager::WlDataDeviceManager =
        source_globals.bind(&source_qh, 1..=3, ())?;
    let _source_keyboard = source_seat.get_keyboard(&source_qh, ());
    let source_data_source = source_manager.create_data_source(&source_qh, ());
    source_data_source.offer("text/plain".to_string());
    source_data_source.offer("text/html".to_string());
    let source_data_device = source_manager.get_data_device(&source_seat, &source_qh, ());
    let source_surface = source_compositor.create_surface(&source_qh, ());
    let source_xdg_surface = source_wm_base.get_xdg_surface(&source_surface, &source_qh, ());
    let _source_toplevel = source_xdg_surface.get_toplevel(&source_qh, ());
    source_surface.commit();
    source_connection.flush()?;

    let mut source_state = RegistryTestState::default();
    source_queue.roundtrip(&mut source_state)?;
    commands.send(ServerCommand::KeyboardKey {
        key: 30,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    source_queue.roundtrip(&mut source_state)?;
    let serial = source_state
        .keyboard_key_serial
        .ok_or_else(|| io::Error::other("keyboard serial was not delivered"))?;
    source_data_device.set_selection(Some(&source_data_source), serial);
    source_connection.flush()?;
    source_connection.roundtrip()?;

    let target_stream = UnixStream::connect(socket_path)?;
    let target_connection = Connection::from_socket(target_stream)?;
    let (target_globals, mut target_queue) =
        registry_queue_init::<RegistryTestState>(&target_connection)?;
    let target_qh = target_queue.handle();

    let target_compositor: client_wl_compositor::WlCompositor =
        target_globals.bind(&target_qh, 1..=6, ())?;
    let target_wm_base: client_xdg_wm_base::XdgWmBase =
        target_globals.bind(&target_qh, 1..=6, ())?;
    let target_seat: client_wl_seat::WlSeat = target_globals.bind(&target_qh, 1..=7, ())?;
    let target_manager: client_wl_data_device_manager::WlDataDeviceManager =
        target_globals.bind(&target_qh, 1..=3, ())?;
    let _target_keyboard = target_seat.get_keyboard(&target_qh, ());
    let target_surface = target_compositor.create_surface(&target_qh, ());
    let target_xdg_surface = target_wm_base.get_xdg_surface(&target_surface, &target_qh, ());
    let _target_toplevel = target_xdg_surface.get_toplevel(&target_qh, ());
    target_surface.commit();
    target_connection.flush()?;

    let mut target_state = RegistryTestState::default();
    target_queue.roundtrip(&mut target_state)?;
    let _target_data_device = target_manager.get_data_device(&target_seat, &target_qh, ());
    target_connection.flush()?;
    target_queue.roundtrip(&mut target_state)?;

    let offer = target_state
        .data_device_selection_offer
        .clone()
        .ok_or_else(|| io::Error::other("target did not receive a clipboard selection offer"))?;
    let (read_fd, write_fd) = owned_pipe()?;
    offer.receive("text/plain".to_string(), write_fd.as_fd());
    target_connection.flush()?;
    drop(write_fd);
    target_connection.roundtrip()?;
    source_queue.roundtrip(&mut source_state)?;

    let mut received = String::new();
    File::from(read_fd).read_to_string(&mut received)?;

    Ok((source_state, target_state, received))
}

fn disconnect_clipboard_source_after_target_offer(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let source_stream = UnixStream::connect(socket_path)?;
    let source_connection = Connection::from_socket(source_stream)?;
    let (source_globals, mut source_queue) =
        registry_queue_init::<RegistryTestState>(&source_connection)?;
    let source_qh = source_queue.handle();

    let source_compositor: client_wl_compositor::WlCompositor =
        source_globals.bind(&source_qh, 1..=6, ())?;
    let source_wm_base: client_xdg_wm_base::XdgWmBase =
        source_globals.bind(&source_qh, 1..=6, ())?;
    let source_seat: client_wl_seat::WlSeat = source_globals.bind(&source_qh, 1..=7, ())?;
    let source_manager: client_wl_data_device_manager::WlDataDeviceManager =
        source_globals.bind(&source_qh, 1..=3, ())?;
    let source_keyboard = source_seat.get_keyboard(&source_qh, ());
    let source_data_source = source_manager.create_data_source(&source_qh, ());
    source_data_source.offer("text/plain".to_string());
    let source_data_device = source_manager.get_data_device(&source_seat, &source_qh, ());
    let source_surface = source_compositor.create_surface(&source_qh, ());
    let source_xdg_surface = source_wm_base.get_xdg_surface(&source_surface, &source_qh, ());
    let source_toplevel = source_xdg_surface.get_toplevel(&source_qh, ());
    source_surface.commit();
    source_connection.flush()?;

    let mut source_state = RegistryTestState::default();
    source_queue.roundtrip(&mut source_state)?;
    commands.send(ServerCommand::KeyboardKey {
        key: 30,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    source_queue.roundtrip(&mut source_state)?;
    let serial = source_state
        .keyboard_key_serial
        .ok_or_else(|| io::Error::other("keyboard serial was not delivered"))?;
    source_data_device.set_selection(Some(&source_data_source), serial);
    source_connection.flush()?;
    source_connection.roundtrip()?;

    let target_stream = UnixStream::connect(socket_path)?;
    let target_connection = Connection::from_socket(target_stream)?;
    let (target_globals, mut target_queue) =
        registry_queue_init::<RegistryTestState>(&target_connection)?;
    let target_qh = target_queue.handle();

    let target_compositor: client_wl_compositor::WlCompositor =
        target_globals.bind(&target_qh, 1..=6, ())?;
    let target_wm_base: client_xdg_wm_base::XdgWmBase =
        target_globals.bind(&target_qh, 1..=6, ())?;
    let target_seat: client_wl_seat::WlSeat = target_globals.bind(&target_qh, 1..=7, ())?;
    let target_manager: client_wl_data_device_manager::WlDataDeviceManager =
        target_globals.bind(&target_qh, 1..=3, ())?;
    let _target_keyboard = target_seat.get_keyboard(&target_qh, ());
    let target_surface = target_compositor.create_surface(&target_qh, ());
    let target_xdg_surface = target_wm_base.get_xdg_surface(&target_surface, &target_qh, ());
    let _target_toplevel = target_xdg_surface.get_toplevel(&target_qh, ());
    target_surface.commit();
    target_connection.flush()?;

    let mut target_state = RegistryTestState::default();
    target_queue.roundtrip(&mut target_state)?;
    let _target_data_device = target_manager.get_data_device(&target_seat, &target_qh, ());
    target_connection.flush()?;
    target_queue.roundtrip(&mut target_state)?;
    assert!(target_state.data_device_selection_offer.is_some());

    drop(source_toplevel);
    drop(source_xdg_surface);
    drop(source_surface);
    drop(source_data_device);
    drop(source_data_source);
    drop(source_keyboard);
    drop(source_manager);
    drop(source_seat);
    drop(source_wm_base);
    drop(source_compositor);
    drop(source_globals);
    drop(source_qh);
    drop(source_queue);
    drop(source_connection);
    wait_for_server_commands(commands);
    wait_for_server_commands(commands);
    target_queue.roundtrip(&mut target_state)?;
    Ok(target_state)
}

fn receive_host_clipboard_from_bridge(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(RegistryTestState, String), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ())?;
    let _keyboard = seat.get_keyboard(&qh, ());
    let _device = manager.get_data_device(&seat, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::KeyboardKey {
        key: 30,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let offer = state
        .data_device_selection_offer
        .clone()
        .ok_or_else(|| io::Error::other("target did not receive host clipboard offer"))?;
    let (read_fd, write_fd) = owned_pipe()?;
    offer.receive("text/plain".to_string(), write_fd.as_fd());
    connection.flush()?;
    drop(write_fd);
    connection.roundtrip()?;

    let mut received = String::new();
    File::from(read_fd).read_to_string(&mut received)?;
    Ok((state, received))
}

#[derive(Debug)]
struct ScriptedClipboardBridge {
    events: VecDeque<ClipboardBridgeEvent>,
    host_payload: &'static [u8],
    requests: Arc<Mutex<Vec<(HostClipboardOfferId, String)>>>,
}

impl ScriptedClipboardBridge {
    fn with_host_selection(
        offer_id: HostClipboardOfferId,
        mime_types: Vec<String>,
        host_payload: &'static [u8],
        requests: Arc<Mutex<Vec<(HostClipboardOfferId, String)>>>,
    ) -> Self {
        Self {
            events: VecDeque::from([ClipboardBridgeEvent::HostSelectionChanged {
                offer_id,
                mime_types,
            }]),
            host_payload,
            requests,
        }
    }
}

impl ClipboardBridge for ScriptedClipboardBridge {
    fn poll_events(&mut self) -> Vec<ClipboardBridgeEvent> {
        self.events.drain(..).collect()
    }

    fn request_host_data(
        &mut self,
        offer_id: HostClipboardOfferId,
        mime_type: String,
        fd: OwnedFd,
    ) -> Result<(), ClipboardBridgeError> {
        self.requests
            .lock()
            .unwrap()
            .push((offer_id, mime_type));
        File::from(fd)
            .write_all(self.host_payload)
            .map_err(|_| ClipboardBridgeError::Unavailable)
    }

    fn publish_internal_selection(
        &mut self,
        _generation: u64,
        _mime_types: Vec<String>,
    ) -> Result<(), ClipboardBridgeError> {
        Ok(())
    }

    fn clear_internal_selection(&mut self) -> Result<(), ClipboardBridgeError> {
        Ok(())
    }
}

fn owned_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }

    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn create_dmabuf_candidate_and_expect_created(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=3, ())?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    let file = create_test_shm_file(&[0xffff_ffff, 0xffff_ffff, 0xffff_ffff, 0xffff_ffff])?;
    let params = dmabuf.create_params(&qh, ());
    params.add(file.as_fd(), 0, 0, 8, 0, 0);
    params.create(
        2,
        2,
        DRM_FORMAT_ARGB8888,
        client_zwp_linux_buffer_params_v1::Flags::empty(),
    );
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn request_dmabuf_default_feedback(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 4..=4, ())?;
    let _feedback = dmabuf.get_default_feedback(&qh, ());
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn request_wl_drm_capabilities(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    request_wl_drm_at_version(socket_path, 2)
}

fn request_wl_drm_at_version(
    socket_path: &PathBuf,
    version: u32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let drm: client_wl_drm::WlDrm = globals.bind(&qh, version..=version, ())?;
    drm.authenticate(0);
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn import_invalid_syncobj_timeline(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let file = File::open("/dev/null")?;
    let _timeline = syncobj.import_timeline(file.as_fd(), &qh, ());
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(())
}

fn set_syncobj_acquire_after_surface_destroy(
    socket_path: &PathBuf,
    timeline: &DrmSyncobjTimeline,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ())?;

    let surface = compositor.create_surface(&qh, ());
    let sync_surface = syncobj.get_surface(&surface, &qh, ());
    let timeline_fd = timeline.export_timeline_fd()?;
    let sync_timeline = syncobj.import_timeline(timeline_fd.as_fd(), &qh, ());
    surface.destroy();
    sync_surface.set_acquire_point(&sync_timeline, 0, 1);
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(())
}

fn create_focused_toplevel_and_receive_key(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let _keyboard = seat.get_keyboard(&qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::KeyboardKey {
        key: 30,
        pressed: true,
    })?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_focused_toplevel_without_keypress(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let _keyboard = seat.get_keyboard(&qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_focused_toplevel_then_press_tab(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let _keyboard = seat.get_keyboard(&qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::KeyboardKey {
        key: 15,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_focused_toplevel_and_receive_two_keys(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let _keyboard = seat.get_keyboard(&qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::KeyboardKey {
        key: 30,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::KeyboardKey {
        key: 30,
        pressed: false,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_focused_toplevel_and_receive_ctrl_modified_key(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let _keyboard = seat.get_keyboard(&qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::KeyboardKey {
        key: 29,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::KeyboardKey {
        key: 30,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_focused_toplevel_and_receive_pointer_motion(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    create_focused_toplevel_and_receive_pointer_motion_at_seat_version(socket_path, commands, 7)
}

fn create_focused_toplevel_and_receive_pointer_motion_at_seat_version(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    seat_version: u32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, seat_version..=seat_version, ())?;
    let _pointer = seat.get_pointer(&qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_focused_toplevel_and_receive_relative_pointer_motion(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    sample: PointerMotionSample,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let pointer = seat.get_pointer(&qh, ());
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let _relative_pointer = relative_manager.get_relative_pointer(&pointer, &qh, ());

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    state.pointer_motion = false;
    state.pointer_surface_x = None;
    state.pointer_surface_y = None;

    commands.send(ServerCommand::PointerMotionSample(sample))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_locked_focused_toplevel_and_receive_pointer_motion_sample(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    sample: PointerMotionSample,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let pointer = seat.get_pointer(&qh, ());
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let _relative_pointer = relative_manager.get_relative_pointer(&pointer, &qh, ());

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::ActivatePointerConstraint(
        PointerConstraintMode::Locked,
    ))?;
    wait_for_server_commands(commands);
    state.pointer_motion = false;
    state.pointer_surface_x = None;
    state.pointer_surface_y = None;

    commands.send(ServerCommand::PointerMotionSample(sample))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn capture_pointer_constraint_backend_requests(
    commands: &Sender<ServerCommand>,
) -> Vec<PointerConstraintBackendRequest> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePointerConstraintBackendRequests(reply))
        .unwrap();
    receiver.recv().unwrap()
}

fn request_lock_activate_and_receive_pointer_motion_sample(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    sample: PointerMotionSample,
) -> Result<(RegistryTestState, Vec<PointerConstraintBackendRequest>), Box<dyn std::error::Error>>
{
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let pointer = seat.get_pointer(&qh, ());
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let _relative_pointer = relative_manager.get_relative_pointer(&pointer, &qh, ());
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ())?;

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let _lock = constraints.lock_pointer(
        &surface,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let requests = capture_pointer_constraint_backend_requests(commands);
    assert_eq!(state.locked_count, 0);
    let backend_id = requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateLocked { id, .. } => Some(*id),
            _ => None,
        })
        .ok_or("expected locked backend activation request")?;
    commands.send(ServerCommand::PointerConstraintBackendActivated(backend_id))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    assert_eq!(state.locked_count, 1);

    state.pointer_motion = false;
    state.pointer_surface_x = None;
    state.pointer_surface_y = None;
    commands.send(ServerCommand::PointerMotionSample(sample))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok((state, requests))
}

fn create_late_pointer_after_focus(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 5..=5, ())?;
    let _pointer_a = seat.get_pointer(&qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    assert_eq!(state.pointer_enter_count, 1);

    let _pointer_b = seat.get_pointer(&qh, ());
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok(state)
}

fn late_pointer_lock_activate_and_receive_relative_motion(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    sample: PointerMotionSample,
) -> Result<(RegistryTestState, Vec<PointerConstraintBackendRequest>), Box<dyn std::error::Error>>
{
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 5..=5, ())?;
    let _pointer_a = seat.get_pointer(&qh, ());
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ())?;

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    assert_eq!(state.pointer_enter_count, 1);

    let pointer_b = seat.get_pointer(&qh, ());
    let _relative_pointer = relative_manager.get_relative_pointer(&pointer_b, &qh, ());
    let _lock = constraints.lock_pointer(
        &surface,
        &pointer_b,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let requests = capture_pointer_constraint_backend_requests(commands);
    assert_eq!(state.locked_count, 0);
    let backend_id = requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateLocked { id, .. } => Some(*id),
            _ => None,
        })
        .ok_or("expected locked backend activation request")?;
    commands.send(ServerCommand::PointerConstraintBackendActivated(backend_id))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    state.pointer_motion = false;
    state.pointer_surface_x = None;
    state.pointer_surface_y = None;
    commands.send(ServerCommand::PointerMotionSample(sample))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok((state, requests))
}

fn lock_activation_repairs_missing_source_pointer_enter_state(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    sample: PointerMotionSample,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 5..=5, ())?;
    let _pointer_a = seat.get_pointer(&qh, ());
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let pointer_b = seat.get_pointer(&qh, ());
    let _relative_pointer = relative_manager.get_relative_pointer(&pointer_b, &qh, ());
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    assert_eq!(state.pointer_enter_count, 2);

    commands.send(ServerCommand::ClearPointerEnterTracking)?;
    wait_for_server_commands(commands);

    let _lock = constraints.lock_pointer(
        &surface,
        &pointer_b,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let requests = capture_pointer_constraint_backend_requests(commands);
    let backend_id = requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateLocked { id, .. } => Some(*id),
            _ => None,
        })
        .ok_or("expected locked backend activation request")?;
    commands.send(ServerCommand::PointerConstraintBackendActivated(backend_id))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::PointerMotionSample(sample))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok(state)
}

fn relative_motion_for_focused_client_is_not_broadcast_to_other_client(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let focused_stream = UnixStream::connect(socket_path)?;
    let focused_connection = Connection::from_socket(focused_stream)?;
    let (focused_globals, mut focused_queue) =
        registry_queue_init::<RegistryTestState>(&focused_connection)?;
    let focused_qh = focused_queue.handle();
    let focused_compositor: client_wl_compositor::WlCompositor =
        focused_globals.bind(&focused_qh, 1..=6, ())?;
    let focused_wm_base: client_xdg_wm_base::XdgWmBase =
        focused_globals.bind(&focused_qh, 1..=6, ())?;
    let focused_seat: client_wl_seat::WlSeat = focused_globals.bind(&focused_qh, 5..=5, ())?;
    let _focused_pointer = focused_seat.get_pointer(&focused_qh, ());
    let focused_surface = focused_compositor.create_surface(&focused_qh, ());
    let focused_xdg_surface = focused_wm_base.get_xdg_surface(&focused_surface, &focused_qh, ());
    let _focused_toplevel = focused_xdg_surface.get_toplevel(&focused_qh, ());
    focused_surface.commit();
    focused_connection.flush()?;

    let mut focused_state = RegistryTestState::default();
    focused_queue.roundtrip(&mut focused_state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    focused_queue.roundtrip(&mut focused_state)?;
    assert_eq!(focused_state.pointer_enter_count, 1);

    let other_stream = UnixStream::connect(socket_path)?;
    let other_connection = Connection::from_socket(other_stream)?;
    let (other_globals, mut other_queue) =
        registry_queue_init::<RegistryTestState>(&other_connection)?;
    let other_qh = other_queue.handle();
    let other_seat: client_wl_seat::WlSeat = other_globals.bind(&other_qh, 5..=5, ())?;
    let other_pointer = other_seat.get_pointer(&other_qh, ());
    let other_relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        other_globals.bind(&other_qh, 1..=1, ())?;
    let _other_relative_pointer =
        other_relative_manager.get_relative_pointer(&other_pointer, &other_qh, ());
    other_connection.flush()?;
    wait_for_server_commands(commands);

    commands.send(ServerCommand::PointerMotionSample(PointerMotionSample {
        timestamp_usec: 44,
        absolute: None,
        relative: Some(RelativePointerMotion {
            dx: 1.0,
            dy: 1.0,
            dx_unaccelerated: 1.0,
            dy_unaccelerated: 1.0,
        }),
    }))?;
    wait_for_server_commands(commands);
    let mut other_state = RegistryTestState::default();
    other_queue.roundtrip(&mut other_state)?;

    Ok(other_state)
}

fn create_focused_toplevel_and_receive_pointer_axis(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let _pointer = seat.get_pointer(&qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerAxis {
        horizontal: 0.0,
        vertical: 15.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_toplevel_then_click_and_move_pointer_on_same_surface(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let _pointer = seat.get_pointer(&qh, ());
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 24.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 18.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_toplevel_then_set_and_commit_cursor_surface(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<usize, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let pointer = seat.get_pointer(&qh, ());
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let Some(serial) = state.pointer_enter_serial else {
        return Err("expected pointer enter serial before set_cursor".into());
    };
    let cursor_surface = compositor.create_surface(&qh, ());
    pointer.set_cursor(serial, Some(&cursor_surface), 1, 1);
    commit_test_buffered_surface(&cursor_surface, &shm, &qh, 24, 24)?;
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok(capture_renderable_surface_count(commands))
}

fn create_buffered_toplevel_and_receive_surface_local_pointer_motion(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let _pointer = seat.get_pointer(&qh, ());

    let width = 100;
    let height = 80;
    let pixels = vec![0xff20_3040; width * height];
    let file = create_test_shm_file(&pixels)?;
    let pool = shm.create_pool(file.as_fd(), (pixels.len() * 4) as i32, &qh, ());
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        (width * 4) as i32,
        client_wl_shm::Format::Argb8888,
        &qh,
        (),
    );

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, width as i32, height as i32);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    })?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_toplevel_with_empty_input_subsurface_and_click_overlap(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    create_toplevel_with_custom_input_subsurface_and_click_overlap(socket_path, commands, None)
}

fn create_toplevel_with_custom_input_subsurface_and_click_overlap(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    child_region: Option<(i32, i32, i32, i32)>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let _pointer = seat.get_pointer(&qh, ());

    let (parent, _xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120)?;
    let parent_surface_id = parent.id().protocol_id();
    toplevel.set_app_id("oblivion.input-region-parent".to_string());
    parent.commit();

    let child = compositor.create_surface(&qh, ());
    let child_surface_id = child.id().protocol_id();
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.set_position(0, 0);
    let region = compositor.create_region(&qh, ());
    if let Some((x, y, width, height)) = child_region {
        region.add(x, y, width, height);
    }
    child.set_input_region(Some(&region));
    commit_test_buffered_surface(&child, &shm, &qh, 160, 120)?;
    connection.flush()?;

    let mut state = RegistryTestState {
        parent_surface_id: Some(parent_surface_id),
        child_surface_id: Some(child_surface_id),
        ..RegistryTestState::default()
    };
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_min_size_toplevel_then_shrink_resize_before_client_commit(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 320, 220)?;
    toplevel.set_min_size(280, 180);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 324.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 224.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 214.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 114.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_scaled_buffer_toplevel_then_right_edge_shrink_and_commit(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.set_buffer_scale(2);
    attach_test_buffered_surface(&surface, &shm, &qh, 600, 400)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 100.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 264.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 100.0,
    })?;
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    surface.set_buffer_scale(2);
    commit_test_buffered_surface(&surface, &shm, &qh, 520, 400)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_scaled_buffer_toplevel_then_left_edge_shrink_and_commit(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.set_buffer_scale(2);
    attach_test_buffered_surface(&surface, &shm, &qh, 600, 400)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) - 3.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 100.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 37.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 100.0,
    })?;
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    surface.set_buffer_scale(2);
    commit_test_buffered_surface(&surface, &shm, &qh, 520, 400)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_toplevel_then_map_subsurface_before_button_release(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let _pointer = seat.get_pointer(&qh, ());

    let (parent, _xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120)?;
    let parent_surface_id = parent.id().protocol_id();
    toplevel.set_app_id("oblivion.implicit-grab-parent".to_string());
    parent.commit();
    connection.flush()?;

    let mut state = RegistryTestState {
        parent_surface_id: Some(parent_surface_id),
        ..RegistryTestState::default()
    };
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let child = compositor.create_surface(&qh, ());
    let child_surface_id = child.id().protocol_id();
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.set_position(0, 0);
    commit_test_buffered_surface(&child, &shm, &qh, 160, 120)?;
    connection.flush()?;
    state.child_surface_id = Some(child_surface_id);
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: false,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_overlapping_subsurfaces_then_place_above_after_parent_commit(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let _pointer = seat.get_pointer(&qh, ());

    let (parent, _xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120)?;
    toplevel.set_app_id("oblivion.subsurface-place-above".to_string());
    parent.commit();

    let lower = compositor.create_surface(&qh, ());
    let lower_id = lower.id().protocol_id();
    let lower_subsurface = subcompositor.get_subsurface(&lower, &parent, &qh, ());
    lower_subsurface.set_position(0, 0);
    commit_test_buffered_surface(&lower, &shm, &qh, 80, 80)?;

    let upper = compositor.create_surface(&qh, ());
    let upper_id = upper.id().protocol_id();
    let upper_subsurface = subcompositor.get_subsurface(&upper, &parent, &qh, ());
    upper_subsurface.set_position(0, 0);
    commit_test_buffered_surface(&upper, &shm, &qh, 81, 81)?;
    connection.flush()?;

    let mut state = RegistryTestState {
        child_surface_id: Some(lower_id),
        second_child_surface_id: Some(upper_id),
        ..RegistryTestState::default()
    };
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    assert_eq!(state.pointer_button_surface_id, Some(upper_id));

    lower_subsurface.place_above(&upper);
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    state.pointer_button_surface_id = None;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: false,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    assert_eq!(state.pointer_button_surface_id, Some(upper_id));

    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    state.pointer_button_surface_id = None;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_subsurface_below_parent_and_click_overlap(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let _pointer = seat.get_pointer(&qh, ());

    let (parent, _xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120)?;
    let parent_id = parent.id().protocol_id();
    toplevel.set_app_id("oblivion.subsurface-place-below-parent".to_string());
    parent.commit();

    let child = compositor.create_surface(&qh, ());
    let child_id = child.id().protocol_id();
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.set_position(0, 0);
    commit_test_buffered_surface(&child, &shm, &qh, 160, 120)?;
    subsurface.place_below(&parent);
    parent.commit();
    connection.flush()?;

    let mut state = RegistryTestState {
        parent_surface_id: Some(parent_id),
        child_surface_id: Some(child_id),
        ..RegistryTestState::default()
    };
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerButton {
        button: 0x110,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_subsurface_with_invalid_restack_reference(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let parent = compositor.create_surface(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let unrelated = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.place_above(&unrelated);
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_repeated_restack_then_destroy_subsurface(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<
    (Vec<RenderableSurfaceSnapshot>, Vec<RenderableSurfaceSnapshot>),
    Box<dyn std::error::Error>,
> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let (parent, _xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120)?;
    toplevel.set_app_id("oblivion.subsurface-repeated-reorder".to_string());
    parent.commit();

    let subtree = compositor.create_surface(&qh, ());
    let subtree_subsurface = subcompositor.get_subsurface(&subtree, &parent, &qh, ());
    subtree_subsurface.set_position(0, 0);
    commit_test_buffered_surface(&subtree, &shm, &qh, 80, 80)?;

    let grandchild = compositor.create_surface(&qh, ());
    let grandchild_subsurface = subcompositor.get_subsurface(&grandchild, &subtree, &qh, ());
    grandchild_subsurface.set_position(1, 1);
    commit_test_buffered_surface(&grandchild, &shm, &qh, 40, 40)?;

    let sibling = compositor.create_surface(&qh, ());
    let sibling_subsurface = subcompositor.get_subsurface(&sibling, &parent, &qh, ());
    sibling_subsurface.set_position(0, 0);
    commit_test_buffered_surface(&sibling, &shm, &qh, 81, 81)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    for _ in 0..3 {
        subtree_subsurface.place_above(&sibling);
    }
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    wait_for_server_commands(commands);
    let reordered = capture_renderable_surface_snapshot(commands);

    subtree_subsurface.destroy();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    wait_for_server_commands(commands);
    let after_destroy = capture_renderable_surface_snapshot(commands);

    Ok((reordered, after_destroy))
}

fn create_pointer_enter_with_v5_pointer(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 5..=5, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let _pointer = seat.get_pointer(&qh, ());

    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120)?;
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_decoy_keyboard_then_focused_toplevel_and_receive_key(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let decoy_stream = UnixStream::connect(socket_path)?;
    let decoy_connection = Connection::from_socket(decoy_stream)?;
    let (decoy_globals, mut decoy_queue) =
        registry_queue_init::<RegistryTestState>(&decoy_connection)?;
    let decoy_qh = decoy_queue.handle();
    let decoy_seat: client_wl_seat::WlSeat = decoy_globals.bind(&decoy_qh, 1..=7, ())?;
    let _decoy_keyboard = decoy_seat.get_keyboard(&decoy_qh, ());
    decoy_connection.flush()?;
    let mut decoy_state = RegistryTestState::default();
    decoy_queue.roundtrip(&mut decoy_state)?;

    let focused_stream = UnixStream::connect(socket_path)?;
    let focused_connection = Connection::from_socket(focused_stream)?;
    let (focused_globals, mut focused_queue) =
        registry_queue_init::<RegistryTestState>(&focused_connection)?;
    let focused_qh = focused_queue.handle();
    let compositor: client_wl_compositor::WlCompositor =
        focused_globals.bind(&focused_qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = focused_globals.bind(&focused_qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = focused_globals.bind(&focused_qh, 1..=7, ())?;
    let _keyboard = seat.get_keyboard(&focused_qh, ());
    let surface = compositor.create_surface(&focused_qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &focused_qh, ());
    let _toplevel = xdg_surface.get_toplevel(&focused_qh, ());
    surface.commit();
    focused_connection.flush()?;

    let mut focused_state = RegistryTestState::default();
    focused_queue.roundtrip(&mut focused_state)?;
    commands.send(ServerCommand::KeyboardKey {
        key: 30,
        pressed: true,
    })?;
    focused_queue.roundtrip(&mut focused_state)?;
    Ok(focused_state)
}

fn create_surface_with_frame_callback(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let surface = compositor.create_surface(&qh, ());
    let _callback = surface.frame(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_surface_with_buffer_frame_callback(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    create_surface_with_delayed_buffer_frame_callback(socket_path, commands, Duration::ZERO)
}

fn create_surface_with_unpresented_buffer_frame_callback(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    let _callback = surface.frame(&qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    Ok(())
}

fn create_visible_surface_frame_callback_without_commit_and_present(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;

    let _callback = surface.frame(&qh, ());
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    assert!(!state.frame_done);

    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_visible_surface_frame_callback_without_commit_and_capture_protocol_only(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let _callback = surface.frame(&qh, ());
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    assert!(!state.frame_done);

    Ok(capture_only_pending_surface_frame_callbacks(commands))
}

fn create_surface_with_delayed_buffer_frame_callback(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    before_present_delay: Duration,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    let _callback = surface.frame(&qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    assert!(!state.frame_done);

    thread::sleep(before_present_delay);
    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_surface_with_buffer_release(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    assert_eq!(state.buffer_release_count, 0);

    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_dmabuf_surface_then_replace_buffer(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    create_dmabuf_surface_then_replace_buffer_inner(socket_path, commands, false)
}

fn create_dmabuf_surface_then_replace_buffer_and_present_twice(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    create_dmabuf_surface_then_replace_buffer_inner(socket_path, commands, true)
}

fn create_dmabuf_surface_then_replace_buffer_inner(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    extra_present: bool,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=3, ())?;
    let first_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff11_1111)?;
    let second_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff22_2222)?;

    let surface = compositor.create_surface(&qh, ());
    surface.attach(Some(&first_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    assert_eq!(state.buffer_release_count, 0);

    surface.attach(Some(&second_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    assert_eq!(state.buffer_release_count, 0);
    if extra_present {
        commands.send(ServerCommand::PresentFrame)?;
        queue.roundtrip(&mut state)?;
    }
    Ok(state)
}

fn create_syncobj_dmabuf_surface_and_present(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    acquire_timeline: &DrmSyncobjTimeline,
    release_timeline: &DrmSyncobjTimeline,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=3, ())?;
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ())?;

    let surface = compositor.create_surface(&qh, ());
    let sync_surface = syncobj.get_surface(&surface, &qh, ());
    let acquire_timeline_fd = acquire_timeline.export_timeline_fd()?;
    let release_timeline_fd = release_timeline.export_timeline_fd()?;
    let sync_acquire_timeline = syncobj.import_timeline(acquire_timeline_fd.as_fd(), &qh, ());
    let sync_release_timeline = syncobj.import_timeline(release_timeline_fd.as_fd(), &qh, ());
    let first_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff44_4444)?;
    let second_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff55_5555)?;

    acquire_timeline.signal_point(1)?;
    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 1);
    sync_surface.set_release_point(&sync_release_timeline, 0, 2);
    surface.attach(Some(&first_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    assert!(!release_timeline.point_signaled(2)?);

    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;

    acquire_timeline.signal_point(3)?;
    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 3);
    sync_surface.set_release_point(&sync_release_timeline, 0, 4);
    surface.attach(Some(&second_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    assert!(!release_timeline.point_signaled(2)?);

    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn create_test_dmabuf_buffer(
    dmabuf: &client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
    qh: &QueueHandle<RegistryTestState>,
    pixel: u32,
) -> Result<client_wl_buffer::WlBuffer, Box<dyn std::error::Error>> {
    let file = create_test_shm_file(&[pixel, pixel, pixel, pixel])?;
    let params = dmabuf.create_params(qh, ());
    params.add(file.as_fd(), 0, 0, 8, 0, 0);
    Ok(params.create_immed(
        2,
        2,
        DRM_FORMAT_ARGB8888,
        client_zwp_linux_buffer_params_v1::Flags::empty(),
        qh,
        (),
    ))
}

fn test_syncobj_device() -> Option<DrmSyncobjDevice> {
    DrmSyncobjDevice::open_available()
}

fn create_client_toplevel_with_shm_buffer(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());

    toplevel.set_app_id("oblivion.buffer-test".to_string());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_client_toplevel_with_shm_damage_only_update(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let mut file = create_test_shm_file(&[0xff11_1111, 0xff22_2222, 0xff33_3333, 0xff44_4444])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());

    toplevel.set_app_id("oblivion.damage-only-test".to_string());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;

    file.seek(SeekFrom::Start(0))?;
    for pixel in [0xffaa_0000_u32, 0xff00_aa00, 0xff00_00aa, 0xffaa_aa00] {
        file.write_all(&pixel.to_ne_bytes())?;
    }
    file.flush()?;
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_client_toplevel_with_dmabuf_buffer(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=3, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let params = dmabuf.create_params(&qh, ());
    params.add(file.as_fd(), 0, 0, 8, 0, 0);
    let buffer = params.create_immed(
        2,
        2,
        DRM_FORMAT_ARGB8888,
        client_zwp_linux_buffer_params_v1::Flags::empty(),
        &qh,
        (),
    );
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());

    toplevel.set_app_id("oblivion.dmabuf-test".to_string());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_toplevel_with_resized_shm_pool_buffer(
    socket_path: &PathBuf,
    resized_pool_size: i32,
    buffer_offset: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[
        0xffff_0000,
        0xff00_ff00,
        0xff00_00ff,
        0xffff_ffff,
        0xff55_0000,
        0xff00_5500,
        0xff00_0055,
        0xff55_5555,
    ])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    pool.resize(resized_pool_size);
    let buffer = pool.create_buffer(
        buffer_offset,
        2,
        2,
        8,
        client_wl_shm::Format::Argb8888,
        &qh,
        (),
    );
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());

    toplevel.set_app_id("oblivion.shm-resize-test".to_string());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn resize_shm_pool_to_invalid_size(
    socket_path: &PathBuf,
    resized_pool_size: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    pool.resize(resized_pool_size);
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_shm_pool_with_invalid_size(
    socket_path: &PathBuf,
    pool_size: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000])?;
    let _pool = shm.create_pool(file.as_fd(), pool_size, &qh, ());
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_client_toplevel_with_shm_then_dmabuf_buffer(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=3, ())?;

    let shm_file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(shm_file.as_fd(), 16, &qh, ());
    let shm_buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());

    let dmabuf_file = create_test_shm_file(&[0xff11_1111, 0xff22_2222, 0xff33_3333, 0xff44_4444])?;
    let params = dmabuf.create_params(&qh, ());
    params.add(dmabuf_file.as_fd(), 0, 0, 8, 0, 0);
    let dmabuf_buffer = params.create_immed(
        2,
        2,
        DRM_FORMAT_ARGB8888,
        client_zwp_linux_buffer_params_v1::Flags::empty(),
        &qh,
        (),
    );

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    toplevel.set_app_id("oblivion.dmabuf-switch-test".to_string());

    surface.attach(Some(&shm_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;

    surface.attach(Some(&dmabuf_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_client_toplevel_with_sized_shm_buffer(
    socket_path: &PathBuf,
    width: usize,
    height: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    create_client_toplevel_with_app_id_and_sized_shm_buffer(
        socket_path,
        "oblivion.buffer-test",
        width,
        height,
    )
}

fn create_client_toplevel_with_app_id_and_sized_shm_buffer(
    socket_path: &PathBuf,
    app_id: &str,
    width: usize,
    height: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, width, height)?;

    toplevel.set_app_id(app_id.to_string());
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_two_live_client_toplevels_and_capture_surface_count(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<usize, Box<dyn std::error::Error>> {
    let first = LiveTestClient::connect(socket_path)?;
    first.create_toplevel("oblivion.client-one", 2, 2)?;
    let second = LiveTestClient::connect(socket_path)?;
    second.create_toplevel("oblivion.client-two", 3, 2)?;
    wait_for_server_commands(commands);
    let count = capture_renderable_surface_count(commands);
    drop((first, second));
    Ok(count)
}

struct LiveTestClient {
    connection: Connection,
    queue: wayland_client::EventQueue<RegistryTestState>,
    compositor: client_wl_compositor::WlCompositor,
    wm_base: client_xdg_wm_base::XdgWmBase,
    shm: client_wl_shm::WlShm,
}

impl LiveTestClient {
    fn connect(socket_path: &PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let stream = UnixStream::connect(socket_path)?;
        let connection = Connection::from_socket(stream)?;
        let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
        let qh = queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
        let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
        let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
        Ok(Self {
            connection,
            queue,
            compositor,
            wm_base,
            shm,
        })
    }

    fn create_toplevel(
        &self,
        app_id: &str,
        width: usize,
        height: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let _surface = self.create_toplevel_surface(app_id, width, height)?;
        Ok(())
    }

    fn create_toplevel_surface(
        &self,
        app_id: &str,
        width: usize,
        height: usize,
    ) -> Result<client_wl_surface::WlSurface, Box<dyn std::error::Error>> {
        let qh = self.queue.handle();
        let (surface, _xdg_surface, toplevel) = create_test_buffered_toplevel(
            &self.compositor,
            &self.wm_base,
            &self.shm,
            &qh,
            width,
            height,
        )?;
        toplevel.set_app_id(app_id.to_string());
        surface.commit();
        self.connection.flush()?;
        self.connection.roundtrip()?;
        Ok(surface)
    }

    fn commit_surface(
        &self,
        surface: &client_wl_surface::WlSurface,
        width: usize,
        height: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let qh = self.queue.handle();
        commit_test_buffered_surface(surface, &self.shm, &qh, width, height)?;
        self.connection.flush()?;
        self.connection.roundtrip()?;
        Ok(())
    }
}

fn create_test_buffered_toplevel(
    compositor: &client_wl_compositor::WlCompositor,
    wm_base: &client_xdg_wm_base::XdgWmBase,
    shm: &client_wl_shm::WlShm,
    qh: &QueueHandle<RegistryTestState>,
    width: usize,
    height: usize,
) -> Result<
    (
        client_wl_surface::WlSurface,
        client_xdg_surface::XdgSurface,
        client_xdg_toplevel::XdgToplevel,
    ),
    Box<dyn std::error::Error>,
> {
    let surface = compositor.create_surface(qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, qh, ());
    let toplevel = xdg_surface.get_toplevel(qh, ());
    attach_test_buffered_surface(&surface, shm, qh, width, height)?;
    Ok((surface, xdg_surface, toplevel))
}

fn commit_test_buffered_surface(
    surface: &client_wl_surface::WlSurface,
    shm: &client_wl_shm::WlShm,
    qh: &QueueHandle<RegistryTestState>,
    width: usize,
    height: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    attach_test_buffered_surface(surface, shm, qh, width, height)?;
    surface.commit();
    Ok(())
}

fn attach_test_buffered_surface(
    surface: &client_wl_surface::WlSurface,
    shm: &client_wl_shm::WlShm,
    qh: &QueueHandle<RegistryTestState>,
    width: usize,
    height: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let pixels = vec![0xff20_3040; width * height];
    let file = create_test_shm_file(&pixels)?;
    let pool = shm.create_pool(file.as_fd(), (pixels.len() * 4) as i32, qh, ());
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        (width * 4) as i32,
        client_wl_shm::Format::Argb8888,
        qh,
        (),
    );
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, width as i32, height as i32);
    Ok(())
}

fn create_client_toplevel_with_positioned_subsurface_buffer(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent_file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let parent_pool = shm.create_pool(parent_file.as_fd(), 16, &qh, ());
    let parent_buffer =
        parent_pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());

    let child_file = create_test_shm_file(&[0xffff_ffff])?;
    let child_pool = shm.create_pool(child_file.as_fd(), 4, &qh, ());
    let child_buffer =
        child_pool.create_buffer(0, 1, 1, 4, client_wl_shm::Format::Argb8888, &qh, ());

    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());

    subsurface.set_position(10, 12);
    parent.attach(Some(&parent_buffer), 0, 0);
    parent.damage_buffer(0, 0, 2, 2);
    child.attach(Some(&child_buffer), 0, 0);
    child.damage_buffer(0, 0, 1, 1);
    child.commit();
    parent.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_subsurface_buffer_before_parent_buffer(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());

    subsurface.set_position(0, 0);
    commit_test_buffered_surface(&child, &shm, &qh, 1, 1)?;
    commit_test_buffered_surface(&parent, &shm, &qh, 2, 2)?;
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_toplevel_then_attach_null_buffer(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    commit_test_buffered_surface(&surface, &shm, &qh, 2, 2)?;
    surface.attach(None, 0, 0);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_toplevel_with_nested_subsurfaces_then_attach_null_buffer(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let grandchild = compositor.create_surface(&qh, ());
    let child_subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    let grandchild_subsurface = subcompositor.get_subsurface(&grandchild, &child, &qh, ());

    child_subsurface.set_position(10, 12);
    grandchild_subsurface.set_position(3, 4);
    commit_test_buffered_surface(&grandchild, &shm, &qh, 1, 1)?;
    commit_test_buffered_surface(&child, &shm, &qh, 1, 1)?;
    commit_test_buffered_surface(&parent, &shm, &qh, 2, 2)?;
    parent.attach(None, 0, 0);
    parent.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

fn create_test_shm_file(pixels: &[u32]) -> Result<File, Box<dyn std::error::Error>> {
    let path = runtime_socket_path(&format!("oblivion-one-shm-{}", unique_socket_name()));
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    fs::remove_file(path)?;
    for pixel in pixels {
        file.write_all(&pixel.to_ne_bytes())?;
    }
    file.flush()?;
    Ok(file)
}

fn spawn_test_server(
    mut server: OwnCompositorServer,
) -> (Arc<AtomicBool>, JoinHandle<OwnCompositorServer>) {
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let server_thread = thread::spawn(move || {
        while server_running.load(Ordering::Relaxed) {
            let _ = server.tick();
            thread::sleep(Duration::from_millis(2));
        }
        server
    });

    (running, server_thread)
}

fn stop_test_server(
    running: Arc<AtomicBool>,
    server_thread: JoinHandle<OwnCompositorServer>,
) -> OwnCompositorServer {
    running.store(false, Ordering::Relaxed);
    server_thread.join().unwrap()
}

#[derive(Clone)]
enum ServerCommand {
    KeyboardKey { key: u32, pressed: bool },
    PointerMotion { x: f64, y: f64 },
    PointerMotionSample(PointerMotionSample),
    ActivatePointerConstraint(PointerConstraintMode),
    PointerButton { button: u32, pressed: bool },
    PointerAxis { horizontal: f64, vertical: f64 },
    BeginFrameAction { x: f64, y: f64 },
    BeginMove { x: f64, y: f64 },
    BeginResize { x: f64, y: f64 },
    UpdateInteraction { x: f64, y: f64 },
    UpdateInteractionResult { x: f64, y: f64, reply: Sender<bool> },
    EndInteraction,
    ResizeFocusedTo { width: u32, height: u32 },
    SetOutputSize { width: u32, height: u32 },
    SetOutputRefresh { refresh_hz: u32 },
    SetOutputScale { scale_factor: f64 },
    MinimizeFocused,
    RestoreNextMinimized,
    ToggleMaximizeFocused,
    ToggleFullscreenFocused,
    CaptureRenderGeneration(Sender<u64>),
    CaptureRenderGenerationCause(Sender<RenderGenerationCause>),
    CaptureRenderableSurfaceCount(Sender<usize>),
    CaptureRenderableSurfaceSnapshot(Sender<Vec<RenderableSurfaceSnapshot>>),
    CaptureXdgRoleSnapshot {
        surface_id: u32,
        reply: Sender<XdgRoleSnapshot>,
    },
    CapturePendingFrameCallbacks(Sender<bool>),
    CaptureOnlyPendingSurfaceFrameCallbacks(Sender<bool>),
    CapturePendingFrameWork(Sender<bool>),
    CaptureIdleInhibited(Sender<bool>),
    CapturePointerConstraintBackendRequests(Sender<Vec<PointerConstraintBackendRequest>>),
    CapturePointerConstraintIds(Sender<Vec<u64>>),
    CaptureLastPointerPosition(Sender<(f64, f64)>),
    PointerConstraintBackendActivated(PointerConstraintBackendId),
    PointerConstraintBackendFailed(PointerConstraintBackendId),
    #[allow(dead_code)]
    PointerConstraintBackendDeactivated(PointerConstraintBackendId),
    ClearPointerEnterTracking,
    Barrier(Sender<()>),
    PrepareFrame,
    FinishFrame,
    FinishFrameWithPresentation(FramePresentation),
    PresentFrame,
    Stop,
}

fn spawn_controllable_test_server(
    mut server: OwnCompositorServer,
) -> (Sender<ServerCommand>, JoinHandle<OwnCompositorServer>) {
    let (commands, receiver) = mpsc::channel();
    let server_thread = thread::spawn(move || {
        let mut running = true;
        while running {
            while let Ok(command) = receiver.try_recv() {
                match command {
                    ServerCommand::KeyboardKey { key, pressed } => {
                        server.send_keyboard_key(key, pressed);
                    }
                    ServerCommand::PointerMotion { x, y } => {
                        server.send_pointer_motion(x, y);
                    }
                    ServerCommand::PointerMotionSample(sample) => {
                        server.send_pointer_motion_sample(sample);
                    }
                    ServerCommand::ActivatePointerConstraint(mode) => {
                        server.state.activate_pointer_constraint_for_focused_surface(mode);
                    }
                    ServerCommand::PointerButton { button, pressed } => {
                        server.send_pointer_button(button, pressed);
                    }
                    ServerCommand::PointerAxis {
                        horizontal,
                        vertical,
                    } => {
                        server.send_pointer_axis(horizontal, vertical);
                    }
                    ServerCommand::BeginFrameAction { x, y } => {
                        server.begin_window_frame_action_at(x, y);
                    }
                    ServerCommand::BeginMove { x, y } => {
                        server.begin_window_move_at(x, y);
                    }
                    ServerCommand::BeginResize { x, y } => {
                        server.begin_window_resize_at(x, y);
                    }
                    ServerCommand::UpdateInteraction { x, y } => {
                        server.update_window_interaction(x, y);
                    }
                    ServerCommand::UpdateInteractionResult { x, y, reply } => {
                        let _ = reply.send(server.update_window_interaction(x, y));
                    }
                    ServerCommand::EndInteraction => {
                        server.end_window_interaction();
                    }
                    ServerCommand::ResizeFocusedTo { width, height } => {
                        server.resize_focused_window_to(width, height);
                    }
                    ServerCommand::SetOutputSize { width, height } => {
                        server.set_output_size(width, height);
                    }
                    ServerCommand::SetOutputRefresh { refresh_hz } => {
                        server.set_output_refresh_hz(refresh_hz);
                    }
                    ServerCommand::SetOutputScale { scale_factor } => {
                        server.set_output_scale_factor(scale_factor);
                    }
                    ServerCommand::MinimizeFocused => {
                        server.minimize_focused_window();
                    }
                    ServerCommand::RestoreNextMinimized => {
                        server.restore_next_minimized_window();
                    }
                    ServerCommand::ToggleMaximizeFocused => {
                        server.toggle_maximize_focused_window();
                    }
                    ServerCommand::ToggleFullscreenFocused => {
                        server.toggle_fullscreen_focused_window();
                    }
                    ServerCommand::CaptureRenderGeneration(reply) => {
                        let _ = reply.send(server.render_generation());
                    }
                    ServerCommand::CaptureRenderGenerationCause(reply) => {
                        let _ = reply.send(server.render_generation_cause());
                    }
                    ServerCommand::CaptureRenderableSurfaceCount(reply) => {
                        let _ = reply.send(server.renderable_surfaces().len());
                    }
                    ServerCommand::CaptureRenderableSurfaceSnapshot(reply) => {
                        let _ = reply.send(
                            server
                                .renderable_surfaces()
                                .iter()
                                .map(|surface| RenderableSurfaceSnapshot {
                                    surface_id: surface.surface_id,
                                    width: surface.width,
                                    height: surface.height,
                                    parent_surface_id: surface.placement.parent_surface_id,
                                })
                                .collect(),
                        );
                    }
                    ServerCommand::CaptureXdgRoleSnapshot { surface_id, reply } => {
                        let tracked_surface_id =
                            if server.state.toplevel_surfaces.len() == 1 {
                                *server.state.toplevel_surfaces.keys().next().unwrap()
                            } else if server.state.surface_resources.contains_key(&surface_id) {
                                surface_id
                            } else if server.state.surface_resources.len() == 1 {
                                *server.state.surface_resources.keys().next().unwrap()
                            } else {
                                surface_id
                            };
                        let _ = reply.send(XdgRoleSnapshot {
                            surface_registered: server
                                .state
                                .surface_resources
                                .contains_key(&tracked_surface_id),
                            configured: server
                                .state
                                .configured_xdg_surfaces
                                .contains(&tracked_surface_id),
                            toplevel_count: server.state.toplevel_surfaces.len(),
                            toplevel_registered: server
                                .state
                                .toplevel_surfaces
                                .contains_key(&tracked_surface_id),
                            popup_count: server.state.popup_surfaces.len(),
                            window_geometry_present: server
                                .state
                                .surface_window_geometries
                                .contains_key(&tracked_surface_id),
                            placement: server
                                .state
                                .surface_placements
                                .get(&tracked_surface_id)
                                .copied(),
                        });
                    }
                    ServerCommand::CapturePendingFrameCallbacks(reply) => {
                        let _ = reply.send(server.has_pending_frame_callbacks());
                    }
                    ServerCommand::CaptureOnlyPendingSurfaceFrameCallbacks(reply) => {
                        let _ = reply.send(server.has_only_pending_surface_frame_callbacks());
                    }
                    ServerCommand::CapturePendingFrameWork(reply) => {
                        let _ = reply.send(server.has_pending_frame_work());
                    }
                    ServerCommand::CaptureIdleInhibited(reply) => {
                        let _ = reply.send(server.state.idle_inhibited());
                    }
                    ServerCommand::CapturePointerConstraintBackendRequests(reply) => {
                        let _ = reply.send(server.take_pointer_constraint_backend_requests());
                    }
                    ServerCommand::CapturePointerConstraintIds(reply) => {
                        let ids = server
                            .state
                            .pointer_constraints
                            .keys()
                            .copied()
                            .collect();
                        let _ = reply.send(ids);
                    }
                    ServerCommand::CaptureLastPointerPosition(reply) => {
                        let _ =
                            reply.send((server.state.last_pointer_x, server.state.last_pointer_y));
                    }
                    ServerCommand::PointerConstraintBackendActivated(id) => {
                        server.pointer_constraint_backend_activated(id);
                    }
                    ServerCommand::PointerConstraintBackendFailed(id) => {
                        server.pointer_constraint_backend_failed(id, "test failure");
                    }
                    ServerCommand::PointerConstraintBackendDeactivated(id) => {
                        server.pointer_constraint_backend_deactivated(id);
                    }
                    ServerCommand::ClearPointerEnterTracking => {
                        server.state.pointer_entered_surfaces.clear();
                    }
                    ServerCommand::Barrier(reply) => {
                        let _ = reply.send(());
                    }
                    ServerCommand::PrepareFrame => {
                        server.prepare_frame();
                    }
                    ServerCommand::FinishFrame => {
                        server.finish_frame();
                    }
                    ServerCommand::FinishFrameWithPresentation(presentation) => {
                        server.finish_frame_with_presentation(presentation);
                    }
                    ServerCommand::PresentFrame => {
                        server.present_frame();
                    }
                    ServerCommand::Stop => running = false,
                }
            }
            let _ = server.tick();
            thread::sleep(Duration::from_millis(2));
        }
        server
    });

    (commands, server_thread)
}

fn stop_controllable_test_server(
    commands: Sender<ServerCommand>,
    server_thread: JoinHandle<OwnCompositorServer>,
) -> OwnCompositorServer {
    let _ = commands.send(ServerCommand::Stop);
    server_thread.join().unwrap()
}

fn wait_for_server_commands(commands: &Sender<ServerCommand>) {
    let (reply, receiver) = mpsc::channel();
    commands.send(ServerCommand::Barrier(reply)).unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should process command barrier");
}

fn capture_render_generation(commands: &Sender<ServerCommand>) -> u64 {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureRenderGeneration(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report render generation")
}

fn capture_render_generation_cause(commands: &Sender<ServerCommand>) -> RenderGenerationCause {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureRenderGenerationCause(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report render generation cause")
}

fn capture_renderable_surface_count(commands: &Sender<ServerCommand>) -> usize {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureRenderableSurfaceCount(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report renderable surface count")
}

fn capture_renderable_surface_snapshot(
    commands: &Sender<ServerCommand>,
) -> Vec<RenderableSurfaceSnapshot> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureRenderableSurfaceSnapshot(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report renderable surface snapshot")
}

fn capture_xdg_role_snapshot(commands: &Sender<ServerCommand>, surface_id: u32) -> XdgRoleSnapshot {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureXdgRoleSnapshot { surface_id, reply })
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report XDG role snapshot")
}

fn capture_pending_frame_callbacks(commands: &Sender<ServerCommand>) -> bool {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePendingFrameCallbacks(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report pending frame callbacks")
}

fn capture_only_pending_surface_frame_callbacks(commands: &Sender<ServerCommand>) -> bool {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureOnlyPendingSurfaceFrameCallbacks(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report pending surface frame callback state")
}

fn capture_pending_frame_work(commands: &Sender<ServerCommand>) -> bool {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePendingFrameWork(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report pending frame work")
}

fn update_interaction_and_report(commands: &Sender<ServerCommand>, x: f64, y: f64) -> bool {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::UpdateInteractionResult { x, y, reply })
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report interaction update")
}

fn runtime_socket_path(socket_name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join(socket_name)
}

fn activate_backend_locked_pointer(
    commands: &Sender<ServerCommand>,
    state: &mut RegistryTestState,
    queue: &mut EventQueue<RegistryTestState>,
) -> Result<PointerConstraintBackendId, Box<dyn std::error::Error>> {
    let requests = capture_pointer_constraint_backend_requests(commands);
    let backend_id = requests
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateLocked { id, .. } => Some(*id),
            _ => None,
        })
        .ok_or("expected locked backend activation request")?;
    commands.send(ServerCommand::PointerConstraintBackendActivated(backend_id))?;
    wait_for_server_commands(commands);
    queue.roundtrip(state)?;
    assert_eq!(state.locked_count, 1);
    Ok(backend_id)
}

fn locked_relative_motion_survives_stale_hit_test(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    sample: PointerMotionSample,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let pointer = seat.get_pointer(&qh, ());
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let _relative_pointer = relative_manager.get_relative_pointer(&pointer, &qh, ());
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ())?;

    let (parent, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120)?;
    let parent_surface_id = parent.id().protocol_id();
    parent.commit();

    let mut state = RegistryTestState {
        parent_surface_id: Some(parent_surface_id),
        ..RegistryTestState::default()
    };
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let _lock = constraints.lock_pointer(
        &parent,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    activate_backend_locked_pointer(commands, &mut state, &mut queue)?;

    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    subsurface.set_position(0, 0);
    let region = compositor.create_region(&qh, ());
    region.add(0, 0, 160, 120);
    child.set_input_region(Some(&region));
    commit_test_buffered_surface(&child, &shm, &qh, 160, 120)?;
    parent.commit();
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let enter_before_motion = state.pointer_enter_count;
    let leave_before_motion = state.pointer_leave_count;
    state.pointer_motion = false;
    state.pointer_surface_x = None;
    state.pointer_surface_y = None;
    commands.send(ServerCommand::PointerMotionSample(sample))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    assert_eq!(state.pointer_enter_count, enter_before_motion);
    assert_eq!(state.pointer_leave_count, leave_before_motion);
    Ok(state)
}

fn run_locked_relative_motion_targets_exact_source_pointer(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    sample: PointerMotionSample,
) -> Result<(RegistryTestState, u32, u32), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 5..=5, ())?;
    let pointer_a = seat.get_pointer(&qh, ());
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ())?;

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let pointer_b = seat.get_pointer(&qh, ());
    let relative_a = relative_manager.get_relative_pointer(&pointer_a, &qh, ());
    let relative_b = relative_manager.get_relative_pointer(&pointer_b, &qh, ());
    let relative_a_id = relative_a.id().protocol_id();
    let relative_b_id = relative_b.id().protocol_id();
    let _lock = constraints.lock_pointer(
        &surface,
        &pointer_b,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    activate_backend_locked_pointer(commands, &mut state, &mut queue)?;

    commands.send(ServerCommand::PointerMotionSample(sample))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok((state, relative_a_id, relative_b_id))
}

fn run_locked_relative_motion_falls_back_to_same_client_pointer_resource(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    sample: PointerMotionSample,
) -> Result<(RegistryTestState, u32), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 5..=5, ())?;
    let pointer_a = seat.get_pointer(&qh, ());
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ())?;

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let pointer_b = seat.get_pointer(&qh, ());
    let relative_a = relative_manager.get_relative_pointer(&pointer_a, &qh, ());
    let relative_a_id = relative_a.id().protocol_id();
    let _lock = constraints.lock_pointer(
        &surface,
        &pointer_b,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    activate_backend_locked_pointer(commands, &mut state, &mut queue)?;

    state.pointer_motion = false;
    state.pointer_surface_x = None;
    state.pointer_surface_y = None;
    commands.send(ServerCommand::PointerMotionSample(sample))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok((state, relative_a_id))
}

fn run_locked_relative_motion_fallback_does_not_cross_clients(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(RegistryTestState, RegistryTestState), Box<dyn std::error::Error>> {
    let locked_stream = UnixStream::connect(socket_path)?;
    let locked_connection = Connection::from_socket(locked_stream)?;
    let (locked_globals, mut locked_queue) =
        registry_queue_init::<RegistryTestState>(&locked_connection)?;
    let locked_qh = locked_queue.handle();
    let locked_compositor: client_wl_compositor::WlCompositor =
        locked_globals.bind(&locked_qh, 1..=6, ())?;
    let locked_wm_base: client_xdg_wm_base::XdgWmBase =
        locked_globals.bind(&locked_qh, 1..=6, ())?;
    let locked_seat: client_wl_seat::WlSeat = locked_globals.bind(&locked_qh, 5..=5, ())?;
    let locked_pointer = locked_seat.get_pointer(&locked_qh, ());
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        locked_globals.bind(&locked_qh, 1..=1, ())?;

    let locked_surface = locked_compositor.create_surface(&locked_qh, ());
    let locked_xdg_surface = locked_wm_base.get_xdg_surface(&locked_surface, &locked_qh, ());
    let _locked_toplevel = locked_xdg_surface.get_toplevel(&locked_qh, ());
    locked_surface.commit();
    locked_connection.flush()?;

    let mut locked_state = RegistryTestState::default();
    locked_queue.roundtrip(&mut locked_state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    locked_queue.roundtrip(&mut locked_state)?;

    let _lock = constraints.lock_pointer(
        &locked_surface,
        &locked_pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &locked_qh,
        (),
    );
    locked_connection.flush()?;
    wait_for_server_commands(commands);
    locked_queue.roundtrip(&mut locked_state)?;
    activate_backend_locked_pointer(commands, &mut locked_state, &mut locked_queue)?;

    let other_stream = UnixStream::connect(socket_path)?;
    let other_connection = Connection::from_socket(other_stream)?;
    let (other_globals, mut other_queue) =
        registry_queue_init::<RegistryTestState>(&other_connection)?;
    let other_qh = other_queue.handle();
    let other_seat: client_wl_seat::WlSeat = other_globals.bind(&other_qh, 5..=5, ())?;
    let other_pointer = other_seat.get_pointer(&other_qh, ());
    let other_relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        other_globals.bind(&other_qh, 1..=1, ())?;
    let _other_relative_pointer =
        other_relative_manager.get_relative_pointer(&other_pointer, &other_qh, ());
    other_connection.flush()?;
    wait_for_server_commands(commands);

    commands
        .send(ServerCommand::PointerMotionSample(PointerMotionSample {
            timestamp_usec: 505,
            absolute: None,
            relative: Some(RelativePointerMotion {
                dx: 3.0,
                dy: -2.0,
                dx_unaccelerated: 3.0,
                dy_unaccelerated: -2.0,
            }),
        }))?;
    wait_for_server_commands(commands);
    let mut other_state = RegistryTestState::default();
    other_queue.roundtrip(&mut other_state)?;

    Ok((locked_state, other_state))
}

fn run_locked_relative_motion_dispatches_to_all_same_client_resources(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    sample: PointerMotionSample,
) -> Result<(RegistryTestState, Vec<u32>), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 5..=5, ())?;
    let pointer_a = seat.get_pointer(&qh, ());
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ())?;

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let pointer_b = seat.get_pointer(&qh, ());
    let relative_a = relative_manager.get_relative_pointer(&pointer_a, &qh, ());
    let relative_b = relative_manager.get_relative_pointer(&pointer_b, &qh, ());
    let expected_ids = vec![relative_a.id().protocol_id(), relative_b.id().protocol_id()];
    let _lock = constraints.lock_pointer(
        &surface,
        &pointer_b,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    activate_backend_locked_pointer(commands, &mut state, &mut queue)?;

    commands.send(ServerCommand::PointerMotionSample(sample))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok((state, expected_ids))
}

fn clear_locked_relative_motion_observations(state: &mut RegistryTestState) {
    state.pointer_frame_count = 0;
    state.pointer_frame_resource_ids.clear();
    state.pointer_event_log.clear();
    state.relative_motion_count = 0;
    state.relative_motion_resource_ids.clear();
    state.relative_motion_utime = None;
    state.relative_motion_dx = None;
    state.relative_motion_dy = None;
    state.relative_motion_dx_unaccel = None;
    state.relative_motion_dy_unaccel = None;
    state.sdl_pending_relative_motion_count = 0;
    state.sdl_camera_motion_count = 0;
    state.pointer_button = false;
}

struct LockedRelativeFrameResult {
    state: RegistryTestState,
    relative_ids: Vec<u32>,
    pointer_ids: Vec<u32>,
}

fn run_locked_relative_motion_shared_source_pointer_frames(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<LockedRelativeFrameResult, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 5..=5, ())?;
    let pointer = seat.get_pointer(&qh, ());
    let pointer_id = pointer.id().protocol_id();
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ())?;

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let relative_a = relative_manager.get_relative_pointer(&pointer, &qh, ());
    let relative_b = relative_manager.get_relative_pointer(&pointer, &qh, ());
    let relative_ids = vec![relative_a.id().protocol_id(), relative_b.id().protocol_id()];
    let _lock = constraints.lock_pointer(
        &surface,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    activate_backend_locked_pointer(commands, &mut state, &mut queue)?;

    clear_locked_relative_motion_observations(&mut state);
    commands.send(ServerCommand::PointerMotionSample(PointerMotionSample {
        timestamp_usec: 808,
        absolute: None,
        relative: Some(RelativePointerMotion {
            dx: 4.0,
            dy: -1.0,
            dx_unaccelerated: 4.0,
            dy_unaccelerated: -1.0,
        }),
    }))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok(LockedRelativeFrameResult {
        state,
        relative_ids,
        pointer_ids: vec![pointer_id],
    })
}

fn run_locked_relative_motion_different_source_pointer_frames(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<LockedRelativeFrameResult, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 5..=5, ())?;
    let pointer_a = seat.get_pointer(&qh, ());
    let pointer_b = seat.get_pointer(&qh, ());
    let pointer_ids = vec![pointer_a.id().protocol_id(), pointer_b.id().protocol_id()];
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ())?;

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion { x: 42.0, y: 48.0 })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let relative_a = relative_manager.get_relative_pointer(&pointer_a, &qh, ());
    let relative_b = relative_manager.get_relative_pointer(&pointer_b, &qh, ());
    let relative_ids = vec![relative_a.id().protocol_id(), relative_b.id().protocol_id()];
    let _lock = constraints.lock_pointer(
        &surface,
        &pointer_b,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    activate_backend_locked_pointer(commands, &mut state, &mut queue)?;

    clear_locked_relative_motion_observations(&mut state);
    commands.send(ServerCommand::PointerMotionSample(PointerMotionSample {
        timestamp_usec: 809,
        absolute: None,
        relative: Some(RelativePointerMotion {
            dx: -2.0,
            dy: 5.0,
            dx_unaccelerated: -2.0,
            dy_unaccelerated: 5.0,
        }),
    }))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok(LockedRelativeFrameResult {
        state,
        relative_ids,
        pointer_ids,
    })
}

fn capture_pointer_constraint_ids(commands: &Sender<ServerCommand>) -> Vec<u64> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePointerConstraintIds(reply))
        .unwrap();
    receiver.recv().unwrap()
}

fn run_multi_client_pointer_constraints_remain_independent(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(PointerConstraintBackendId, PointerConstraintBackendId), Box<dyn std::error::Error>> {
    #[allow(clippy::type_complexity)]
    fn setup_client(
        socket_path: &PathBuf,
    ) -> Result<
        (
            Connection,
            EventQueue<RegistryTestState>,
            client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1,
            client_wl_surface::WlSurface,
            client_wl_pointer::WlPointer,
        ),
        Box<dyn std::error::Error>,
    > {
        let stream = UnixStream::connect(socket_path)?;
        let connection = Connection::from_socket(stream)?;
        let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
        let qh = queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
        let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
        let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
        let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
        let pointer = seat.get_pointer(&qh, ());
        let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
            globals.bind(&qh, 1..=1, ())?;
        let (surface, _xdg_surface, _toplevel) =
            create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 120, 90)?;
        surface.commit();
        connection.flush()?;
        Ok((connection, queue, constraints, surface, pointer))
    }

    let (connection_a, mut queue_a, constraints_a, surface_a, pointer_a) =
        setup_client(socket_path)?;
    let mut state_a = RegistryTestState::default();
    queue_a.roundtrip(&mut state_a)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    })?;
    wait_for_server_commands(commands);
    queue_a.roundtrip(&mut state_a)?;

    let _lock_a = constraints_a.lock_pointer(
        &surface_a,
        &pointer_a,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &queue_a.handle(),
        (),
    );
    connection_a.flush()?;
    wait_for_server_commands(commands);
    queue_a.roundtrip(&mut state_a)?;
    let requests_a = capture_pointer_constraint_backend_requests(commands);
    let id_a = requests_a
        .iter()
        .find_map(|request| match request {
            PointerConstraintBackendRequest::ActivateLocked { id, .. } => Some(*id),
            _ => None,
        })
        .ok_or("expected client A locked backend activation request")?;

    let (connection_b, mut queue_b, constraints_b, surface_b, pointer_b) =
        setup_client(socket_path)?;
    let mut state_b = RegistryTestState::default();
    queue_b.roundtrip(&mut state_b)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 14.0,
    })?;
    wait_for_server_commands(commands);
    queue_b.roundtrip(&mut state_b)?;

    let _lock_b = constraints_b.lock_pointer(
        &surface_b,
        &pointer_b,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &queue_b.handle(),
        (),
    );
    connection_b.flush()?;
    wait_for_server_commands(commands);
    queue_b.roundtrip(&mut state_b)?;
    let ids = capture_pointer_constraint_ids(commands);
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
    assert!(ids.contains(&id_a.constraint_id));

    commands.send(ServerCommand::PointerConstraintBackendActivated(id_a))?;
    wait_for_server_commands(commands);
    queue_a.roundtrip(&mut state_a)?;
    assert_eq!(state_a.locked_count, 1);
    assert_eq!(state_b.locked_count, 0);

    let wrong_client_activation = PointerConstraintBackendId {
        constraint_id: id_a.constraint_id,
        generation: id_a.generation.wrapping_add(999),
    };
    commands.send(ServerCommand::PointerConstraintBackendActivated(
        wrong_client_activation,
    ))?;
    wait_for_server_commands(commands);
    queue_b.roundtrip(&mut state_b)?;
    assert_eq!(state_b.locked_count, 0);

    commands
        .send(ServerCommand::PointerMotionSample(PointerMotionSample {
            timestamp_usec: 1,
            absolute: None,
            relative: Some(RelativePointerMotion {
                dx: 3.0,
                dy: 1.0,
                dx_unaccelerated: 3.0,
                dy_unaccelerated: 1.0,
            }),
        }))?;
    wait_for_server_commands(commands);
    queue_b.roundtrip(&mut state_b)?;
    assert_eq!(state_b.relative_motion_count, 0);

    Ok((id_a, PointerConstraintBackendId {
        constraint_id: ids.iter().copied().find(|id| *id != id_a.constraint_id).unwrap(),
        generation: id_a.generation,
    }))
}

fn run_locked_relative_motion_survives_surface_tree_churn(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    sample: PointerMotionSample,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let pointer = seat.get_pointer(&qh, ());
    let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let _relative_pointer = relative_manager.get_relative_pointer(&pointer, &qh, ());
    let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
        globals.bind(&qh, 1..=1, ())?;

    let (parent, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 160, 120)?;
    parent.commit();

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PointerMotion {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 20.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 20.0,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let _lock = constraints.lock_pointer(
        &parent,
        &pointer,
        None,
        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
        &qh,
        (),
    );
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    activate_backend_locked_pointer(commands, &mut state, &mut queue)?;

    let lower = compositor.create_surface(&qh, ());
    let lower_subsurface = subcompositor.get_subsurface(&lower, &parent, &qh, ());
    lower_subsurface.set_position(0, 0);
    commit_test_buffered_surface(&lower, &shm, &qh, 80, 80)?;

    let upper = compositor.create_surface(&qh, ());
    let upper_subsurface = subcompositor.get_subsurface(&upper, &parent, &qh, ());
    upper_subsurface.set_position(0, 0);
    let region = compositor.create_region(&qh, ());
    region.add(0, 0, 80, 80);
    upper.set_input_region(Some(&region));
    commit_test_buffered_surface(&upper, &shm, &qh, 80, 80)?;
    lower_subsurface.place_above(&upper);
    parent.commit();
    connection.flush()?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::PointerMotionSample(sample))?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok(state)
}

fn unique_socket_name() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("oblivion-one-test-{}-{now}", std::process::id())
}
