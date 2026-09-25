use super::*;
use oblivion_one::compositor::ViewportSourceRect;

#[test]
fn lamp_geometry_cache_tracks_live_viewport_mapping_without_changing_bounds() {
    let lifecycle = lamp_test_sample(0.5);
    let surface = test_shm_surface(RenderableSurfaceDamage::full());
    let before_target =
        compositor::surface_render_space_assignments(std::slice::from_ref(&surface), 1.0)[0].target;
    let before_key = lamp_geometry_key(
        &lifecycle,
        std::slice::from_ref(&surface),
        &[],
        1.0,
        OutputFramebufferOrigin::BottomLeft,
    );

    let mut remapped = surface.clone();
    remapped.viewport_source = Some(ViewportSourceRect {
        x: 0.25,
        y: 0.5,
        width: 1.0,
        height: 1.0,
    });
    remapped.viewport_destination = Some(BufferSize::new(1, 2).expect("viewport destination"));
    let after_target =
        compositor::surface_render_space_assignments(std::slice::from_ref(&remapped), 1.0)[0]
            .target;
    let after_key = lamp_geometry_key(
        &lifecycle,
        std::slice::from_ref(&remapped),
        &[],
        1.0,
        OutputFramebufferOrigin::BottomLeft,
    );

    assert_eq!(after_target, before_target);
    assert_ne!(after_key, before_key);

    let mut clipped = surface.clone();
    clipped.visual_clip = Some(compositor::SurfaceVisualAperture::logical_only(
        compositor::SurfaceTargetRect::new(1, 2, 3, 4),
    ));
    let clipped_target =
        compositor::surface_render_space_assignments(std::slice::from_ref(&clipped), 1.0)[0].target;
    let clipped_key = lamp_geometry_key(
        &lifecycle,
        std::slice::from_ref(&clipped),
        &[],
        1.0,
        OutputFramebufferOrigin::BottomLeft,
    );
    assert_eq!(clipped_target, before_target);
    assert_ne!(clipped_key, before_key);

    let mut content_changed = surface.clone();
    let mut buffer_ids = BufferIdAllocator::default();
    let _ = buffer_ids
        .allocate()
        .expect("first content buffer identity");
    let next_buffer = buffer_ids.allocate().expect("new content buffer identity");
    content_changed.buffer = CommittedSurfaceBuffer::shm_snapshot(
        next_buffer,
        BufferSize::new(2, 2).expect("content buffer size"),
        vec![0xff12_3456; 4],
    );
    content_changed.generation = content_changed.generation.saturating_add(1);
    let content_key = lamp_geometry_key(
        &lifecycle,
        std::slice::from_ref(&content_changed),
        &[],
        1.0,
        OutputFramebufferOrigin::BottomLeft,
    );
    assert_eq!(content_key, before_key);

    remapped.buffer_scale = 2;
    let scaled_key = lamp_geometry_key(
        &lifecycle,
        std::slice::from_ref(&remapped),
        &[],
        1.0,
        OutputFramebufferOrigin::BottomLeft,
    );
    remapped.buffer_transform = wayland_server::protocol::wl_output::Transform::_90;
    let transformed_key = lamp_geometry_key(
        &lifecycle,
        std::slice::from_ref(&remapped),
        &[],
        1.0,
        OutputFramebufferOrigin::BottomLeft,
    );
    assert_ne!(scaled_key, before_key);
    assert_ne!(transformed_key, scaled_key);
}
