use super::dmabuf_import::{
    DmabufImportDiagnosticContext, DmabufImportFailureClass, DmabufImportGlStage, DmabufImportPath,
    DmabufImportRegistryState, DmabufTextureImportError, first_drained_gl_error,
    settle_dmabuf_import_result,
};
use super::dmabuf_registry::DmabufImportRegistry;
use super::image::{EglImageResource, EglImageResourceTelemetry};
use super::surface::{
    EglSurfaceResource, EglSurfaceResourceUpdate, SurfaceResourceLifetimeAction,
    classify_surface_resource_lifetime, reconcile_surface_resource_backing,
};
use super::ui::{DecorationResourceKey, test_decoration_resource_requirements};
use super::*;

use std::collections::HashMap;

use khronos_egl as egl;
use oblivion_one::compositor::{
    RenderableSurface, RenderableSurfaceDamage, SurfaceCommitCounter, SurfaceCommitSequence,
    SurfaceDamageRect, SurfaceOpaqueRegion, SurfacePlacement, SurfaceRenderBackend,
    SurfaceResourceSyncState,
};
use oblivion_one::render_backend::buffer::{
    BufferIdAllocator, BufferIdentity, BufferSize, CommittedSurfaceBuffer, DmabufBufferHandle,
    DmabufImageKey, DmabufPlane, DmabufPlaneDescriptor, DrmFormat, DrmModifier,
};

mod dmabuf;
mod image;
mod shm;
mod ui;

const XR24: u32 = u32::from_le_bytes(*b"XR24");

pub(super) struct DropProbe(pub(super) std::rc::Rc<std::cell::Cell<usize>>);

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.set(self.0.get().saturating_add(1));
    }
}

pub(super) fn test_shm_surface(damage: RenderableSurfaceDamage) -> RenderableSurface {
    let identity = BufferIdAllocator::default()
        .allocate()
        .expect("test buffer identity");
    RenderableSurface {
        surface_id: 7,
        x: 0,
        y: 0,
        width: 2,
        height: 2,
        placement: SurfacePlacement::root(),
        render_backend: SurfaceRenderBackend::NativeWayland,
        render_placement: None,
        visual_clip: None,
        render_target_size: None,
        generation: 2,
        commit_sequence: SurfaceCommitSequence::initial(),
        buffer: CommittedSurfaceBuffer::shm_snapshot(
            identity,
            BufferSize::new(2, 2).expect("test surface size"),
            vec![0xff00_0000; 4],
        ),
        viewport_source: None,
        viewport_destination: None,
        buffer_scale: 1,
        buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
        opaque_region: SurfaceOpaqueRegion::None,
        damage,
    }
}

pub(super) fn test_shm_resource(synced_commit: Option<SurfaceCommitCounter>) -> EglSurfaceResource {
    EglSurfaceResource::Uploaded {
        image: EglImageResource {
            texture: glow::NativeTexture(std::num::NonZeroU32::new(1).unwrap()),
            size: (2, 2),
            generation: 1,
            egl_image: None,
            telemetry: EglImageResourceTelemetry::Untracked,
        },
        shm_synced_commit: synced_commit,
    }
}

pub(super) fn test_dmabuf_binding(key: DmabufImageKey, _generation: u64) -> EglSurfaceResource {
    EglSurfaceResource::Dmabuf { key }
}
