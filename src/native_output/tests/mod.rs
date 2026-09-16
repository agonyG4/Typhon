use super::*;
use crate::native_output::runtime::NativeFrameRequest;
use oblivion_one::compositor::{
    DesktopVisualState, RenderableSurfaceDamage, SurfaceCommitSequence, SurfaceDamageRect,
    SurfacePlacement, compose_output, render_scene_elements_for_surfaces, surface_origins,
};
use oblivion_one::render_backend::buffer::{
    BufferIdAllocator, BufferIdentity, BufferSize, CommittedSurfaceBuffer,
};
use oblivion_one::{CompositorAppGpuPreference, EffectiveCompositorAppGpuPolicy};
use std::sync::{Mutex, OnceLock};

struct ApplicationScopeDisabledGuard {
    previous: Option<std::ffi::OsString>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl Drop for ApplicationScopeDisabledGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => unsafe { std::env::set_var("OBLIVION_ONE_APP_SCOPES", value) },
            None => unsafe { std::env::remove_var("OBLIVION_ONE_APP_SCOPES") },
        }
    }
}

fn disable_application_scopes_for_test() -> ApplicationScopeDisabledGuard {
    let lock = ASTREA_ENV_LOCK.lock().unwrap();
    let previous = std::env::var_os("OBLIVION_ONE_APP_SCOPES");
    unsafe { std::env::set_var("OBLIVION_ONE_APP_SCOPES", "off") };
    ApplicationScopeDisabledGuard {
        previous,
        _lock: lock,
    }
}

fn test_buffer_identity() -> BufferIdentity {
    static IDS: OnceLock<Mutex<BufferIdAllocator>> = OnceLock::new();
    IDS.get_or_init(|| Mutex::new(BufferIdAllocator::default()))
        .lock()
        .expect("test buffer identity allocator")
        .allocate()
        .expect("test buffer identity")
}

mod binding_launch;
mod direct_scanout_stage4;
mod frame;
mod fullscreen_cadence;
mod fullscreen_frame_scene;
mod input;
mod input_interaction_liveness;
mod input_protocol;
mod input_xwayland_client;
mod integrated_swapchain_oracle;
mod output;
mod output_identity;
mod output_retry;
mod plane_scheduling_model;
mod presentation_transactions;
mod scanout;
mod shell_control;
mod special_workspace;
mod triple_buffering_model;
