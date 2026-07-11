use super::*;
use crate::native_output::runtime::NativeFrameRequest;
use oblivion_one::compositor::{
    DesktopVisualState, RenderableSurfaceDamage, SurfaceCommitSequence, SurfaceDamageRect,
    SurfacePlacement, compose_nested_output, render_scene_elements_for_surfaces, surface_origins,
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

mod frame;
mod input;
mod output;
mod scanout;
mod shell_control;
