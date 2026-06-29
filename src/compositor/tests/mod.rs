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
        wl_surface as client_wl_surface,
    },
};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1 as client_wp_fractional_scale_manager_v1,
    wp_fractional_scale_v1 as client_wp_fractional_scale_v1,
};
use wayland_protocols::wp::idle_inhibit::zv1::client::{
    zwp_idle_inhibit_manager_v1 as client_zwp_idle_inhibit_manager_v1,
    zwp_idle_inhibitor_v1 as client_zwp_idle_inhibitor_v1,
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

mod support;

use support::client_setup::*;
use support::clipboard_dmabuf::*;
use support::frame_buffer_client::*;
use support::input_client::*;
use support::locked_relative::*;
use support::output_bindings::*;
use support::registry_state::*;
use support::server_runtime::*;
use support::subsurface_client::*;
use support::window_ops::*;

mod input_output;
mod lifecycle;
mod plan;
mod protocol_buffers;
mod subsurface;
mod surface_frames;
mod windows;
mod xdg;
