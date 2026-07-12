#![allow(unused_imports)]
use super::super::*;
use super::{
    client_setup::*, clipboard_dmabuf::*, frame_buffer_client::*, input_client::*,
    locked_relative::*, output_bindings::*, server_runtime::*, subsurface_client::*, window_ops::*,
};
#[derive(Default)]
pub(in crate::compositor::tests) struct RegistryTestState {
    pub(in crate::compositor::tests) frame_done: bool,
    pub(in crate::compositor::tests) frame_done_time: Option<u32>,
    pub(in crate::compositor::tests) keyboard_key: bool,
    pub(in crate::compositor::tests) keyboard_key_serial: Option<u32>,
    pub(in crate::compositor::tests) keyboard_keys: Vec<u32>,
    pub(in crate::compositor::tests) keyboard_keymap: bool,
    pub(in crate::compositor::tests) keyboard_keymap_bytes: Vec<u8>,
    pub(in crate::compositor::tests) keyboard_keymap_size: u32,
    pub(in crate::compositor::tests) keyboard_mods_depressed: Vec<u32>,
    pub(in crate::compositor::tests) keyboard_repeat_info: bool,
    pub(in crate::compositor::tests) pointer_enter: bool,
    pub(in crate::compositor::tests) pointer_enter_count: usize,
    pub(in crate::compositor::tests) pointer_leave_count: usize,
    pub(in crate::compositor::tests) pointer_enter_serial: Option<u32>,
    pub(in crate::compositor::tests) pointer_enter_serials: Vec<(u32, u32)>,
    pub(in crate::compositor::tests) pointer_motion: bool,
    pub(in crate::compositor::tests) pointer_button: bool,
    pub(in crate::compositor::tests) pointer_button_serial: Option<u32>,
    pub(in crate::compositor::tests) pointer_enter_surface_id: Option<u32>,
    pub(in crate::compositor::tests) pointer_button_surface_id: Option<u32>,
    pub(in crate::compositor::tests) pointer_button_surface_ids: Vec<u32>,
    pub(in crate::compositor::tests) pointer_axis: bool,
    pub(in crate::compositor::tests) pointer_vertical_axis: Option<f64>,
    pub(in crate::compositor::tests) pointer_horizontal_axis: Option<f64>,
    pub(in crate::compositor::tests) pointer_frame_count: usize,
    pub(in crate::compositor::tests) pointer_frame_resource_ids: Vec<u32>,
    pub(in crate::compositor::tests) pointer_enter_frame_count: usize,
    pub(in crate::compositor::tests) pointer_enter_without_frame_count: usize,
    pub(in crate::compositor::tests) pointer_event_log: Vec<&'static str>,
    pub(in crate::compositor::tests) pointer_surface_x: Option<f64>,
    pub(in crate::compositor::tests) pointer_surface_y: Option<f64>,
    pub(in crate::compositor::tests) relative_motion_count: usize,
    pub(in crate::compositor::tests) relative_motion_utime: Option<u64>,
    pub(in crate::compositor::tests) relative_motion_dx: Option<f64>,
    pub(in crate::compositor::tests) relative_motion_dy: Option<f64>,
    pub(in crate::compositor::tests) relative_motion_dx_unaccel: Option<f64>,
    pub(in crate::compositor::tests) relative_motion_dy_unaccel: Option<f64>,
    pub(in crate::compositor::tests) relative_motion_resource_ids: Vec<u32>,
    pub(in crate::compositor::tests) sdl_pending_relative_motion_count: usize,
    pub(in crate::compositor::tests) sdl_camera_motion_count: usize,
    pub(in crate::compositor::tests) locked_count: usize,
    pub(in crate::compositor::tests) unlocked_count: usize,
    pub(in crate::compositor::tests) confined_count: usize,
    pub(in crate::compositor::tests) unconfined_count: usize,
    pub(in crate::compositor::tests) parent_surface_id: Option<u32>,
    pub(in crate::compositor::tests) child_surface_id: Option<u32>,
    pub(in crate::compositor::tests) second_child_surface_id: Option<u32>,
    pub(in crate::compositor::tests) keyboard_enter_surface_id: Option<u32>,
    pub(in crate::compositor::tests) keyboard_enter_count: usize,
    pub(in crate::compositor::tests) keyboard_leave_count: usize,
    pub(in crate::compositor::tests) keyboard_event_log: Vec<&'static str>,
    pub(in crate::compositor::tests) surface_enter_count: usize,
    pub(in crate::compositor::tests) seat_has_keyboard: bool,
    pub(in crate::compositor::tests) output_done: bool,
    pub(in crate::compositor::tests) output_mode_count: usize,
    pub(in crate::compositor::tests) output_scale_count: usize,
    pub(in crate::compositor::tests) output_name: bool,
    pub(in crate::compositor::tests) output_description: bool,
    pub(in crate::compositor::tests) output_width: i32,
    pub(in crate::compositor::tests) output_height: i32,
    pub(in crate::compositor::tests) output_refresh_millihertz: i32,
    pub(in crate::compositor::tests) seat_name: bool,
    pub(in crate::compositor::tests) seat_has_pointer: bool,
    pub(in crate::compositor::tests) surface_configured: bool,
    pub(in crate::compositor::tests) surface_configure_count: usize,
    pub(in crate::compositor::tests) surface_configure_serials: Vec<u32>,
    pub(in crate::compositor::tests) layer_surface_configured: bool,
    pub(in crate::compositor::tests) layer_surface_configure_count: usize,
    pub(in crate::compositor::tests) layer_surface_configure_serials: Vec<u32>,
    pub(in crate::compositor::tests) layer_surface_configures: Vec<(u32, u32, u32)>,
    pub(in crate::compositor::tests) layer_surface_width: u32,
    pub(in crate::compositor::tests) layer_surface_height: u32,
    pub(in crate::compositor::tests) layer_surface_closed: bool,
    pub(in crate::compositor::tests) suppress_layer_surface_ack: bool,
    pub(in crate::compositor::tests) popup_configured: bool,
    pub(in crate::compositor::tests) popup_configure_count: usize,
    pub(in crate::compositor::tests) popup_repositioned_token: Option<u32>,
    pub(in crate::compositor::tests) popup_x: i32,
    pub(in crate::compositor::tests) popup_y: i32,
    pub(in crate::compositor::tests) popup_width: i32,
    pub(in crate::compositor::tests) popup_height: i32,
    pub(in crate::compositor::tests) popup_done: bool,
    pub(in crate::compositor::tests) configured_before_initial_commit: bool,
    pub(in crate::compositor::tests) configured_after_initial_commit: bool,
    pub(in crate::compositor::tests) toplevel_configured: bool,
    pub(in crate::compositor::tests) toplevel_configure_count: usize,
    pub(in crate::compositor::tests) toplevel_width: i32,
    pub(in crate::compositor::tests) toplevel_height: i32,
    pub(in crate::compositor::tests) toplevel_states: Vec<u8>,
    pub(in crate::compositor::tests) dmabuf_modifier: bool,
    pub(in crate::compositor::tests) dmabuf_failed: bool,
    pub(in crate::compositor::tests) dmabuf_created: bool,
    pub(in crate::compositor::tests) dmabuf_feedback_main_device: bool,
    pub(in crate::compositor::tests) dmabuf_feedback_format_table: bool,
    pub(in crate::compositor::tests) dmabuf_feedback_format_table_size: u32,
    pub(in crate::compositor::tests) dmabuf_feedback_tranche_formats: bool,
    pub(in crate::compositor::tests) dmabuf_feedback_done: bool,
    pub(in crate::compositor::tests) wl_drm_device: bool,
    pub(in crate::compositor::tests) wl_drm_capabilities: bool,
    pub(in crate::compositor::tests) wl_drm_format: bool,
    pub(in crate::compositor::tests) wl_drm_authenticated: bool,
    pub(in crate::compositor::tests) buffer_release_count: usize,
    pub(in crate::compositor::tests) presentation_presented_count: usize,
    pub(in crate::compositor::tests) presentation_discarded_count: usize,
    pub(in crate::compositor::tests) presentation_kind:
        Option<client_wp_presentation_feedback::Kind>,
    pub(in crate::compositor::tests) presentation_clock_id: Option<u32>,
    pub(in crate::compositor::tests) presentation_timestamp: Option<(u32, u32, u32)>,
    pub(in crate::compositor::tests) presentation_sequence: Option<(u32, u32)>,
    pub(in crate::compositor::tests) fractional_preferred_scales: Vec<u32>,
    pub(in crate::compositor::tests) data_device_selection_offer:
        Option<client_wl_data_offer::WlDataOffer>,
    pub(in crate::compositor::tests) data_device_selection_events: Vec<bool>,
    pub(in crate::compositor::tests) data_offer_mime_types: Vec<String>,
    pub(in crate::compositor::tests) data_source_send_mime_types: Vec<String>,
    pub(in crate::compositor::tests) data_source_cancelled: bool,
    pub(in crate::compositor::tests) activation_token_done: Option<String>,
    pub(in crate::compositor::tests) astrea_shortcut_pressed_count: usize,
    pub(in crate::compositor::tests) astrea_shortcut_pressed_serials: Vec<u32>,
    pub(in crate::compositor::tests) astrea_shortcut_pressed_timestamps: Vec<u32>,
    pub(in crate::compositor::tests) astrea_shortcut_cancelled_count: usize,
    pub(in crate::compositor::tests) astrea_shortcut_cancelled_serials: Vec<u32>,
    pub(in crate::compositor::tests) astrea_shortcut_events: Vec<AstreaShortcutEventRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor::tests) struct ClipboardStateSnapshot {
    pub(in crate::compositor::tests) active_source: bool,
    pub(in crate::compositor::tests) source_count: usize,
    pub(in crate::compositor::tests) offer_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::compositor::tests) enum AstreaShortcutEventRecord {
    Pressed { serial: u32, timestamp: u32 },
    Repeated { serial: u32, timestamp: u32 },
    Released { serial: u32, timestamp: u32 },
    Cancelled { serial: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::compositor::tests) struct XdgRoleSnapshot {
    pub(in crate::compositor::tests) surface_registered: bool,
    pub(in crate::compositor::tests) configured: bool,
    pub(in crate::compositor::tests) toplevel_count: usize,
    pub(in crate::compositor::tests) toplevel_registered: bool,
    pub(in crate::compositor::tests) popup_count: usize,
    pub(in crate::compositor::tests) popup_node_count: usize,
    pub(in crate::compositor::tests) popup_grab_active: bool,
    pub(in crate::compositor::tests) window_geometry_present: bool,
    pub(in crate::compositor::tests) placement: Option<SurfacePlacement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::compositor::tests) struct RenderableSurfaceSnapshot {
    pub(in crate::compositor::tests) surface_id: u32,
    pub(in crate::compositor::tests) width: u32,
    pub(in crate::compositor::tests) height: u32,
    pub(in crate::compositor::tests) parent_surface_id: Option<u32>,
    pub(in crate::compositor::tests) local_x: i32,
    pub(in crate::compositor::tests) local_y: i32,
    pub(in crate::compositor::tests) origin_x: i32,
    pub(in crate::compositor::tests) origin_y: i32,
    pub(in crate::compositor::tests) buffer_id: u64,
    pub(in crate::compositor::tests) generation: u64,
    pub(in crate::compositor::tests) resize_preview_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor::tests) struct ToplevelVisualGeometrySnapshot {
    pub(in crate::compositor::tests) local_x: i32,
    pub(in crate::compositor::tests) local_y: i32,
    pub(in crate::compositor::tests) width: u32,
    pub(in crate::compositor::tests) height: u32,
    pub(in crate::compositor::tests) active_resize: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::compositor::tests) struct CsdResizeRegressionSnapshot {
    pub(in crate::compositor::tests) toplevel_width: i32,
    pub(in crate::compositor::tests) toplevel_height: i32,
    pub(in crate::compositor::tests) toplevel_configure_count: usize,
    pub(in crate::compositor::tests) surfaces: Vec<RenderableSurfaceSnapshot>,
    pub(in crate::compositor::tests) visual: Option<ToplevelVisualGeometrySnapshot>,
    pub(in crate::compositor::tests) window_geometry: Option<XdgWindowGeometry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::compositor::tests) struct CsdConsecutiveResizeSnapshots {
    pub(in crate::compositor::tests) first_final: CsdResizeRegressionSnapshot,
    pub(in crate::compositor::tests) second_preview: CsdResizeRegressionSnapshot,
    pub(in crate::compositor::tests) second_final: CsdResizeRegressionSnapshot,
    pub(in crate::compositor::tests) third_preview: CsdResizeRegressionSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::compositor::tests) struct CsdTopLeftResizeSnapshot {
    pub(in crate::compositor::tests) first_final: CsdResizeRegressionSnapshot,
    pub(in crate::compositor::tests) top_left_preview: CsdResizeRegressionSnapshot,
}

pub(in crate::compositor::tests) struct WindowGeometryCommitSnapshots {
    pub(in crate::compositor::tests) before_pending: Vec<RenderableSurfaceSnapshot>,
    pub(in crate::compositor::tests) before_pending_geometry: Option<XdgWindowGeometry>,
    pub(in crate::compositor::tests) after_pending_without_commit: Vec<RenderableSurfaceSnapshot>,
    pub(in crate::compositor::tests) after_pending_without_commit_geometry:
        Option<XdgWindowGeometry>,
    pub(in crate::compositor::tests) after_geometry_commit: Vec<RenderableSurfaceSnapshot>,
    pub(in crate::compositor::tests) after_geometry_commit_geometry: Option<XdgWindowGeometry>,
}

pub(in crate::compositor::tests) struct ExplicitSyncWindowGeometrySnapshots {
    pub(in crate::compositor::tests) before_blocked_commit: Vec<RenderableSurfaceSnapshot>,
    pub(in crate::compositor::tests) before_blocked_geometry: Option<XdgWindowGeometry>,
    pub(in crate::compositor::tests) while_acquire_blocked: Vec<RenderableSurfaceSnapshot>,
    pub(in crate::compositor::tests) blocked_geometry: Option<XdgWindowGeometry>,
    pub(in crate::compositor::tests) after_acquire_ready: Vec<RenderableSurfaceSnapshot>,
    pub(in crate::compositor::tests) after_acquire_geometry: Option<XdgWindowGeometry>,
}

pub(in crate::compositor::tests) struct SynchronizedCommitSnapshots {
    pub(in crate::compositor::tests) before_parent: Vec<RenderableSurfaceSnapshot>,
    pub(in crate::compositor::tests) after_parent: Vec<RenderableSurfaceSnapshot>,
    pub(in crate::compositor::tests) before_child_generation: u64,
    pub(in crate::compositor::tests) after_child_generation: u64,
    pub(in crate::compositor::tests) after_parent_generation: u64,
}

pub(in crate::compositor::tests) struct RootBeforeChildSnapshots {
    pub(in crate::compositor::tests) after_root: Vec<RenderableSurfaceSnapshot>,
    pub(in crate::compositor::tests) after_child_without_parent: Vec<RenderableSurfaceSnapshot>,
    pub(in crate::compositor::tests) after_next_parent: Vec<RenderableSurfaceSnapshot>,
}

pub(in crate::compositor::tests) struct GeckoPreRoleAdoptionSnapshots {
    pub(in crate::compositor::tests) after_roleless_commit: Vec<RenderableSurfaceSnapshot>,
    pub(in crate::compositor::tests) after_adoption: Vec<RenderableSurfaceSnapshot>,
}

pub(in crate::compositor::tests) struct MultipleSynchronizedCommitSnapshots {
    pub(in crate::compositor::tests) before_parent: Vec<RenderableSurfaceSnapshot>,
    pub(in crate::compositor::tests) after_parent: Vec<RenderableSurfaceSnapshot>,
    pub(in crate::compositor::tests) superseded_buffer_releases: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor::tests) struct ClientCursorSnapshot {
    pub(in crate::compositor::tests) surface_id: u32,
    pub(in crate::compositor::tests) logical_x: i32,
    pub(in crate::compositor::tests) logical_y: i32,
    pub(in crate::compositor::tests) width: u32,
    pub(in crate::compositor::tests) height: u32,
}

#[derive(Debug)]
pub(in crate::compositor::tests) struct CursorSurfaceCommitSnapshot {
    pub(in crate::compositor::tests) renderable_count: usize,
    pub(in crate::compositor::tests) cursor: Option<ClientCursorSnapshot>,
    pub(in crate::compositor::tests) callback_state: Option<(bool, bool)>,
    pub(in crate::compositor::tests) cause: RenderGenerationCause,
}

#[derive(Debug)]
pub(in crate::compositor::tests) struct CursorTransitionSnapshots {
    pub(in crate::compositor::tests) initial: Option<ClientCursorSnapshot>,
    pub(in crate::compositor::tests) hotspot_changed: Option<ClientCursorSnapshot>,
    pub(in crate::compositor::tests) hidden: Option<ClientCursorSnapshot>,
    pub(in crate::compositor::tests) reselected: Option<ClientCursorSnapshot>,
    pub(in crate::compositor::tests) destroyed: Option<ClientCursorSnapshot>,
}

#[derive(Debug)]
pub(in crate::compositor::tests) struct CompositorOnlyCursorMotionSnapshot {
    pub(in crate::compositor::tests) cursor: Option<ClientCursorSnapshot>,
    pub(in crate::compositor::tests) visual_changed: bool,
    pub(in crate::compositor::tests) render_generation_before: u64,
    pub(in crate::compositor::tests) render_generation_after: u64,
    pub(in crate::compositor::tests) scene_generation_before: u64,
    pub(in crate::compositor::tests) scene_generation_after: u64,
    pub(in crate::compositor::tests) cause: RenderGenerationCause,
    pub(in crate::compositor::tests) pointer_event_log_before: Vec<&'static str>,
    pub(in crate::compositor::tests) pointer_event_log_after: Vec<&'static str>,
    pub(in crate::compositor::tests) relative_motion_count_before: usize,
    pub(in crate::compositor::tests) relative_motion_count_after: usize,
    pub(in crate::compositor::tests) pointer_focus_surface_before: Option<u32>,
    pub(in crate::compositor::tests) pointer_focus_surface_after: Option<u32>,
}

#[derive(Debug)]
pub(in crate::compositor::tests) struct CursorMotionStateSnapshot {
    pub(in crate::compositor::tests) cursor: Option<ClientCursorSnapshot>,
    pub(in crate::compositor::tests) visual_changed: bool,
    pub(in crate::compositor::tests) render_generation: u64,
    pub(in crate::compositor::tests) scene_generation: u64,
    pub(in crate::compositor::tests) cause: RenderGenerationCause,
    pub(in crate::compositor::tests) pointer_event_log: Vec<&'static str>,
    pub(in crate::compositor::tests) pointer_motion_count: usize,
    pub(in crate::compositor::tests) relative_motion_count: usize,
    pub(in crate::compositor::tests) pointer_focus_surface: Option<u32>,
}

#[derive(Debug)]
pub(in crate::compositor::tests) struct CompositorOnlyCursorSynchronizationSnapshots {
    pub(in crate::compositor::tests) initial: CursorMotionStateSnapshot,
    pub(in crate::compositor::tests) compositor_only: CursorMotionStateSnapshot,
    pub(in crate::compositor::tests) interaction: CursorMotionStateSnapshot,
    pub(in crate::compositor::tests) normal_motion: CursorMotionStateSnapshot,
    pub(in crate::compositor::tests) interaction_update_applied: bool,
    pub(in crate::compositor::tests) resize_visual_active: bool,
}

impl RegistryTestState {
    pub(in crate::compositor::tests) fn toplevel_has_state(
        &self,
        expected: client_xdg_toplevel::State,
    ) -> bool {
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

impl Dispatch<client_astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1, ()>
    for RegistryTestState
{
    fn event(
        _state: &mut Self,
        _proxy: &client_astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1,
        _event: client_astrea_shortcuts_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_astrea_shortcut_v1::AstreaShortcutV1, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_astrea_shortcut_v1::AstreaShortcutV1,
        event: client_astrea_shortcut_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            client_astrea_shortcut_v1::Event::Pressed { serial, timestamp } => {
                state.astrea_shortcut_pressed_count += 1;
                state.astrea_shortcut_pressed_serials.push(serial);
                state.astrea_shortcut_pressed_timestamps.push(timestamp);
                state
                    .astrea_shortcut_events
                    .push(AstreaShortcutEventRecord::Pressed { serial, timestamp });
            }
            client_astrea_shortcut_v1::Event::Cancelled { serial } => {
                state.astrea_shortcut_cancelled_count += 1;
                state.astrea_shortcut_cancelled_serials.push(serial);
                state
                    .astrea_shortcut_events
                    .push(AstreaShortcutEventRecord::Cancelled { serial });
            }
            client_astrea_shortcut_v1::Event::Repeated { serial, timestamp } => {
                state
                    .astrea_shortcut_events
                    .push(AstreaShortcutEventRecord::Repeated { serial, timestamp });
            }
            client_astrea_shortcut_v1::Event::Released { serial, timestamp } => {
                state
                    .astrea_shortcut_events
                    .push(AstreaShortcutEventRecord::Released { serial, timestamp });
            }
        }
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
                state.data_device_selection_events.push(id.is_some());
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
            client_wl_pointer::Event::Button {
                serial,
                state: button_state,
                ..
            } => {
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
            state
                .relative_motion_resource_ids
                .push(_proxy.id().protocol_id());
            state.relative_motion_utime = Some((u64::from(utime_hi) << 32) | u64::from(utime_lo));
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

impl Dispatch<client_xdg_activation_v1::XdgActivationV1, ()> for RegistryTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_xdg_activation_v1::XdgActivationV1,
        _event: client_xdg_activation_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_xdg_activation_token_v1::XdgActivationTokenV1, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_xdg_activation_token_v1::XdgActivationTokenV1,
        event: client_xdg_activation_token_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let client_xdg_activation_token_v1::Event::Done { token } = event {
            state.activation_token_done = Some(token);
        }
    }
}

impl Dispatch<client_zwlr_layer_shell_v1::ZwlrLayerShellV1, ()> for RegistryTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_zwlr_layer_shell_v1::ZwlrLayerShellV1,
        _event: client_zwlr_layer_shell_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        proxy: &client_zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: client_zwlr_layer_surface_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            client_zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                state.layer_surface_configured = true;
                state.layer_surface_configure_count += 1;
                state.layer_surface_configure_serials.push(serial);
                state.layer_surface_configures.push((serial, width, height));
                state.layer_surface_width = width;
                state.layer_surface_height = height;
                if !state.suppress_layer_surface_ack {
                    proxy.ack_configure(serial);
                }
            }
            client_zwlr_layer_surface_v1::Event::Closed => {
                state.layer_surface_closed = true;
            }
            _ => {}
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
