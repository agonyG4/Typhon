use super::super::*;

pub(in crate::egl_renderer::tests) fn lamp_test_sample(progress: f64) -> LifecycleSceneSample {
    let rect = PresentationRect::new(100.0, 80.0, 800.0, 600.0).expect("valid rectangle");
    let anchor = PresentationRect::new(1200.0, 900.0, 64.0, 64.0).expect("valid anchor");
    let window_id = oblivion_one::compositor::WindowId::from_raw(1).expect("valid window id");
    let presentation_identity = test_lifecycle_identity(window_id, 1);
    let payload_id =
        oblivion_one::compositor::PresentationRetainedVisualPayloadId::from_origin_identity(
            presentation_identity,
        );
    let visual_group = LifecycleVisualGroup::from_bounds(rect, rect, rect, anchor, 1920, 1080)
        .expect("valid visual group");
    LifecycleSceneSample {
        sampled_at: AnimationTime::from_nanos(1),
        samples: vec![LifecycleWindowSample {
            window_id,
            root_surface_id: 1,
            presentation_identity,
            payload_id,
            visual_group,
            visual_source: LifecycleVisualSource {
                window_id,
                root_surface_id: 1,
                presentation_identity,
                payload_id,
                kind: LifecycleVisualSourceKind::NoOwnedEffects,
                effect_scene: Arc::new(compositor::ResolvedEffectScene::default()),
            },
            effect: LifecycleEffectKind::Lamp,
            progress,
            effect_opacity: 1.0,
            direction: LifecycleDirection::Minimize,
            mathematically_settled: false,
        }],
    }
}

pub(in crate::egl_renderer::tests) fn lifecycle_blur_effect_scene(
    root_surface_id: u32,
    program: EffectProgramId,
) -> ResolvedEffectScene {
    lifecycle_blur_effect_scene_for_rect(
        root_surface_id,
        program,
        EffectRect::new(60, 40, 180, 110).unwrap(),
    )
}

pub(in crate::egl_renderer::tests) fn lifecycle_blur_effect_scene_for_rect(
    root_surface_id: u32,
    program: EffectProgramId,
    owner_rect: EffectRect,
) -> ResolvedEffectScene {
    let region = EffectRegion::from_rect(owner_rect);
    let anchor = compositor::EffectAnchor::BeforeSurface(root_surface_id);
    ResolvedEffectScene::new(
        1,
        vec![compositor::ResolvedEffectInstance {
            id: oblivion_one::effects::EffectInstanceId::new(1).unwrap(),
            program,
            anchor,
            target_bounds: region.bounding_rect().unwrap(),
            region,
            parameter_block: EffectParameterBlock::default(),
            signature: 1,
            frame_demand: EffectFrameDemand::OnDamage,
            visual_group: Some(VisualGroupId::new(1).expect("test visual group id")),
            anchor_scope: compositor::EffectAnchorScope::VisualGroup,
            scene_order: compositor::EffectSceneOrder::for_anchor(anchor),
        }],
    )
}

pub(in crate::egl_renderer::tests) fn lifecycle_effect_sample(
    progress: f64,
    direction: LifecycleDirection,
    root_surface_id: u32,
    started_at: u64,
    effect_scene: ResolvedEffectScene,
) -> LifecycleSceneSample {
    let window_id = oblivion_one::compositor::WindowId::from_raw(1).expect("test window id");
    let presentation_identity = test_lifecycle_identity(window_id, started_at);
    let payload_id =
        oblivion_one::compositor::PresentationRetainedVisualPayloadId::from_origin_identity(
            presentation_identity,
        );
    let source = PresentationRect::new(60.0, 40.0, 180.0, 110.0).expect("source rect");
    let anchor = PresentationRect::new(220.0, 130.0, 60.0, 40.0).expect("anchor rect");
    let visual_group = LifecycleVisualGroup::from_bounds(source, source, source, anchor, 320, 200)
        .expect("valid lifecycle visual group");
    LifecycleSceneSample {
        sampled_at: AnimationTime::from_nanos(started_at),
        samples: vec![LifecycleWindowSample {
            window_id,
            root_surface_id,
            presentation_identity,
            payload_id,
            visual_group,
            visual_source: LifecycleVisualSource {
                window_id,
                root_surface_id,
                presentation_identity,
                payload_id,
                kind: LifecycleVisualSourceKind::ResolvedOwnedEffects,
                effect_scene: Arc::new(effect_scene),
            },
            effect: LifecycleEffectKind::Lamp,
            progress,
            effect_opacity: 1.0,
            direction,
            mathematically_settled: false,
        }],
    }
}

pub(in crate::egl_renderer::tests) fn lifecycle_test_surface(
    surface_id: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    color: u32,
    buffer_ids: &mut BufferIdAllocator,
) -> RenderableSurface {
    RenderableSurface {
        surface_id,
        x,
        y,
        width,
        height,
        placement: SurfacePlacement::root(),
        render_backend: SurfaceRenderBackend::NativeWayland,
        render_placement: None,
        visual_clip: None,
        render_target_size: None,
        generation: 1,
        commit_sequence: SurfaceCommitSequence::initial(),
        buffer: CommittedSurfaceBuffer::shm_snapshot(
            buffer_ids.allocate().expect("test buffer identity"),
            BufferSize::new(width, height).expect("test surface size"),
            vec![color; width as usize * height as usize],
        ),
        viewport_source: None,
        viewport_destination: None,
        buffer_scale: 1,
        buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
        damage: RenderableSurfaceDamage::full(),
        opaque_region: SurfaceOpaqueRegion::None,
    }
}

pub(in crate::egl_renderer::tests) fn lifecycle_test_surface_with_row_colors(
    surface_id: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    row_colors: [u32; 3],
    buffer_ids: &mut BufferIdAllocator,
) -> RenderableSurface {
    let mut surface =
        lifecycle_test_surface(surface_id, x, y, width, height, row_colors[0], buffer_ids);
    let pixels = (0..height)
        .flat_map(|row| {
            let band = ((row * 3) / height).min(2) as usize;
            std::iter::repeat_n(row_colors[band], width as usize)
        })
        .collect();
    surface.buffer = CommittedSurfaceBuffer::shm_snapshot(
        buffer_ids.allocate().expect("test buffer identity"),
        BufferSize::new(width, height).expect("test surface size"),
        pixels,
    );
    surface
}
