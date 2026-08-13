#![allow(dead_code)]

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
        wl_data_offer as client_wl_data_offer, wl_data_source as client_wl_data_source,
        wl_keyboard as client_wl_keyboard, wl_output as client_wl_output,
        wl_pointer as client_wl_pointer, wl_region as client_wl_region, wl_registry,
        wl_seat as client_wl_seat, wl_shm as client_wl_shm, wl_shm_pool as client_wl_shm_pool,
        wl_subcompositor as client_wl_subcompositor, wl_subsurface as client_wl_subsurface,
        wl_surface as client_wl_surface, wl_touch as client_wl_touch,
    },
};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1 as client_ext_data_control_device_v1,
    ext_data_control_manager_v1 as client_ext_data_control_manager_v1,
    ext_data_control_offer_v1 as client_ext_data_control_offer_v1,
    ext_data_control_source_v1 as client_ext_data_control_source_v1,
};
use wayland_protocols::wp::commit_timing::v1::client::{
    wp_commit_timer_v1 as client_wp_commit_timer_v1,
    wp_commit_timing_manager_v1 as client_wp_commit_timing_manager_v1,
};
use wayland_protocols::wp::content_type::v1::client::{
    wp_content_type_manager_v1 as client_wp_content_type_manager_v1,
    wp_content_type_v1 as client_wp_content_type_v1,
};
use wayland_protocols::wp::fifo::v1::client::{
    wp_fifo_manager_v1 as client_wp_fifo_manager_v1, wp_fifo_v1 as client_wp_fifo_v1,
};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1 as client_wp_fractional_scale_manager_v1,
    wp_fractional_scale_v1 as client_wp_fractional_scale_v1,
};
use wayland_protocols::wp::idle_inhibit::zv1::client::{
    zwp_idle_inhibit_manager_v1 as client_zwp_idle_inhibit_manager_v1,
    zwp_idle_inhibitor_v1 as client_zwp_idle_inhibitor_v1,
};
use wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::client::{
    zwp_keyboard_shortcuts_inhibit_manager_v1 as client_zwp_keyboard_shortcuts_inhibit_manager_v1,
    zwp_keyboard_shortcuts_inhibitor_v1 as client_zwp_keyboard_shortcuts_inhibitor_v1,
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
use wayland_protocols::wp::pointer_constraints::zv1::client::{
    zwp_confined_pointer_v1 as client_zwp_confined_pointer_v1,
    zwp_locked_pointer_v1 as client_zwp_locked_pointer_v1,
    zwp_pointer_constraints_v1 as client_zwp_pointer_constraints_v1,
};
use wayland_protocols::wp::pointer_warp::v1::client::wp_pointer_warp_v1 as client_wp_pointer_warp_v1;
use wayland_protocols::wp::presentation_time::client::{
    wp_presentation as client_wp_presentation,
    wp_presentation_feedback as client_wp_presentation_feedback,
};
use wayland_protocols::wp::primary_selection::zv1::client::{
    zwp_primary_selection_device_manager_v1 as client_zwp_primary_selection_device_manager_v1,
    zwp_primary_selection_device_v1 as client_zwp_primary_selection_device_v1,
    zwp_primary_selection_offer_v1 as client_zwp_primary_selection_offer_v1,
    zwp_primary_selection_source_v1 as client_zwp_primary_selection_source_v1,
};
use wayland_protocols::wp::relative_pointer::zv1::client::{
    zwp_relative_pointer_manager_v1 as client_zwp_relative_pointer_manager_v1,
    zwp_relative_pointer_v1 as client_zwp_relative_pointer_v1,
};
use wayland_protocols::wp::tearing_control::v1::client::{
    wp_tearing_control_manager_v1 as client_wp_tearing_control_manager_v1,
    wp_tearing_control_v1 as client_wp_tearing_control_v1,
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport as client_wp_viewport, wp_viewporter as client_wp_viewporter,
};
use wayland_protocols::xdg::activation::v1::client::{
    xdg_activation_token_v1 as client_xdg_activation_token_v1,
    xdg_activation_v1 as client_xdg_activation_v1,
};
use wayland_protocols::xdg::shell::client::{
    xdg_popup as client_xdg_popup, xdg_positioner as client_xdg_positioner,
    xdg_surface as client_xdg_surface, xdg_toplevel as client_xdg_toplevel,
    xdg_wm_base as client_xdg_wm_base,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1 as client_zwlr_layer_shell_v1,
    zwlr_layer_surface_v1 as client_zwlr_layer_surface_v1,
};

use crate::astrea_shell_auth::client::astrea_shell_auth_manager_v1 as client_astrea_shell_auth_manager_v1;
use crate::astrea_shortcuts::client::{
    astrea_shortcut_v1 as client_astrea_shortcut_v1,
    astrea_shortcuts_manager_v1 as client_astrea_shortcuts_manager_v1,
};

mod output_model;
mod support;

use support::client_setup::*;
use support::clipboard_dmabuf::*;
use support::frame_buffer_client::*;
use support::input_client::*;
use support::locked_relative::*;
use support::output_bindings::*;
use support::protocol_errors::*;
use support::registry_state::*;
use support::server_runtime::*;
use support::subsurface_client::*;
use support::window_ops::*;

mod astrea_shell_auth;
mod astrea_shell_capability;
mod astrea_shortcuts;
mod data_control;
mod data_device;
mod direct_scanout;
mod frame_pacing;
mod input_output;
mod layer_shell;
mod lifecycle;
mod native_geometry;
mod plan;
mod presentation_modes;
mod primary_selection;
mod protocol_buffers;
mod protocol_contract;
mod protocol_error;
mod subsurface;
mod surface_frames;
mod toplevel_management;
mod windows;
mod windows_geometry;
mod windows_resize_liveness;
mod xdg;
mod xwayland;
