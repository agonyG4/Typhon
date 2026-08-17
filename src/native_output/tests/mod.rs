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
mod fullscreen_frame_scene;
mod fullscreen_cadence;
mod input;
mod input_interaction_liveness;
mod input_protocol;
mod input_xwayland_client;
mod output;
mod output_retry;
mod plane_scheduling_model;
mod presentation_transactions;
mod scanout;
mod shell_control;
mod triple_buffering_model;
