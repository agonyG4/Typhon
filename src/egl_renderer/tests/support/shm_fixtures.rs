use super::super::*;

pub(in crate::egl_renderer::tests) fn test_shm_surface(
    damage: RenderableSurfaceDamage,
) -> RenderableSurface {
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

pub(in crate::egl_renderer::tests) fn test_shm_resource(
    synced_commit: Option<SurfaceCommitCounter>,
) -> EglSurfaceResource {
    EglSurfaceResource {
        image: EglImageResource {
            texture: glow::NativeTexture(std::num::NonZeroU32::new(1).unwrap()),
            size: (2, 2),
            generation: 1,
            egl_image: None,
        },
        dmabuf_key: None,
        buffer_lifetime: None,
        shm_synced_commit: synced_commit,
    }
}
