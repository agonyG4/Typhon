use super::super::*;

pub(in crate::egl_renderer::tests) fn moving_blur_scene(
    rect: EffectRect,
) -> (ResolvedEffectScene, EffectRegistry) {
    moving_blur_scene_with_spec(rect, 4.0, 2, 1.0)
}

pub(in crate::egl_renderer::tests) fn moving_blur_scene_with_spec(
    rect: EffectRect,
    radius: f32,
    passes: u8,
    scale: f32,
) -> (ResolvedEffectScene, EffectRegistry) {
    let source = EffectNodeId::new(1).expect("source id");
    let blur = EffectNodeId::new(2).expect("blur id");
    let program = validate_effect_program(EffectProgram {
        id: EffectProgramId::new(1).expect("program id"),
        nodes: vec![
            EffectNode::source(source, EffectSource::Backdrop),
            EffectNode::dual_kawase(
                blur,
                source,
                DualKawaseBlurSpec::new(radius, passes, scale).expect("blur spec"),
            ),
        ],
        output: blur,
        working_space: EffectWorkingSpace::LinearSrgb,
        alpha_mode: EffectAlphaMode::Opaque,
        outsets: oblivion_one::effects::EffectOutsets::ZERO,
        frame_demand: EffectFrameDemand::OnDamage,
        failure_policy: EffectFailurePolicy::Passthrough,
    })
    .expect("moving blur program validates");
    let mut registry = EffectRegistry::empty();
    registry
        .insert(program)
        .expect("moving blur program inserts");
    let instance = oblivion_one::compositor::ResolvedEffectInstance {
        id: oblivion_one::effects::EffectInstanceId::new(1).expect("instance id"),
        program: EffectProgramId::new(1).expect("program id"),
        anchor: oblivion_one::compositor::EffectAnchor::OutputPostProcess,
        target_bounds: rect,
        region: oblivion_one::effects::EffectRegion::from_rect(rect),
        parameter_block: EffectParameterBlock::default(),
        signature: 11,
        frame_demand: EffectFrameDemand::OnDamage,
        visual_group: None,
        anchor_scope: oblivion_one::compositor::EffectAnchorScope::VisualGroup,
        scene_order: oblivion_one::compositor::EffectSceneOrder::for_anchor(
            oblivion_one::compositor::EffectAnchor::OutputPostProcess,
        ),
    };
    (ResolvedEffectScene::new(1, vec![instance]), registry)
}

pub(in crate::egl_renderer::tests) fn moving_visual_group_blur_scene(
    rect: EffectRect,
    visual_group: VisualGroupId,
) -> ResolvedEffectScene {
    moving_visual_group_blur_scene_with_spec(rect, visual_group, 4.0, 2, 1.0)
}

pub(in crate::egl_renderer::tests) fn moving_visual_group_blur_scene_with_spec(
    rect: EffectRect,
    visual_group: VisualGroupId,
    radius: f32,
    passes: u8,
    scale: f32,
) -> ResolvedEffectScene {
    let (mut scene, _) = moving_blur_scene_with_spec(rect, radius, passes, scale);
    let instance = scene.instances.first_mut().expect("moving blur instance");
    instance.anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(42);
    instance.visual_group = Some(visual_group);
    instance.anchor_scope = oblivion_one::compositor::EffectAnchorScope::VisualGroup;
    instance.scene_order = oblivion_one::compositor::EffectSceneOrder::for_anchor(instance.anchor);
    scene
}

#[derive(Clone, Copy)]
pub(in crate::egl_renderer::tests) struct NativeStackedBackdropEffectSpec {
    pub(in crate::egl_renderer::tests) id: u64,
    pub(in crate::egl_renderer::tests) target_bounds: EffectRect,
    pub(in crate::egl_renderer::tests) anchor: oblivion_one::compositor::EffectAnchor,
    pub(in crate::egl_renderer::tests) anchor_scope: oblivion_one::compositor::EffectAnchorScope,
    pub(in crate::egl_renderer::tests) visual_group: Option<VisualGroupId>,
    pub(in crate::egl_renderer::tests) scene_order: oblivion_one::compositor::EffectSceneOrder,
}

pub(in crate::egl_renderer::tests) fn native_backdrop_effect_instance(
    spec: NativeStackedBackdropEffectSpec,
    program: EffectProgramId,
) -> oblivion_one::compositor::ResolvedEffectInstance {
    oblivion_one::compositor::ResolvedEffectInstance {
        id: oblivion_one::effects::EffectInstanceId::new(spec.id)
            .expect("native backdrop instance id"),
        program,
        anchor: spec.anchor,
        target_bounds: spec.target_bounds,
        region: EffectRegion::from_rect(spec.target_bounds),
        parameter_block: EffectParameterBlock::default(),
        signature: 11_u64.saturating_add(spec.id),
        frame_demand: EffectFrameDemand::OnDamage,
        visual_group: spec.visual_group,
        anchor_scope: spec.anchor_scope,
        scene_order: spec.scene_order,
    }
}

pub(in crate::egl_renderer::tests) fn native_stacked_backdrop_scene(
    a: NativeStackedBackdropEffectSpec,
    b: NativeStackedBackdropEffectSpec,
    radius: f32,
    passes: u8,
    scale: f32,
) -> (ResolvedEffectScene, EffectRegistry) {
    let (program_scene, registry) =
        moving_blur_scene_with_spec(a.target_bounds, radius, passes, scale);
    let program = program_scene
        .instances
        .first()
        .expect("native backdrop program instance")
        .program;
    let first = native_backdrop_effect_instance(a, program);
    let second = native_backdrop_effect_instance(b, program);
    (ResolvedEffectScene::new(1, vec![first, second]), registry)
}

#[derive(Clone, Copy)]
pub(in crate::egl_renderer::tests) struct NativeThreeCheckpointSceneSpec {
    pub(in crate::egl_renderer::tests) background_surface: u32,
    pub(in crate::egl_renderer::tests) a_surface: u32,
    pub(in crate::egl_renderer::tests) c_surface: u32,
    pub(in crate::egl_renderer::tests) b_surface: u32,
    pub(in crate::egl_renderer::tests) a_content_bounds: EffectRect,
    pub(in crate::egl_renderer::tests) c_content_bounds: EffectRect,
    pub(in crate::egl_renderer::tests) b_content_bounds: EffectRect,
    pub(in crate::egl_renderer::tests) a_visual_group: VisualGroupId,
    pub(in crate::egl_renderer::tests) c_visual_group: VisualGroupId,
    pub(in crate::egl_renderer::tests) b_visual_group: VisualGroupId,
}

#[derive(Clone, Copy)]
pub(in crate::egl_renderer::tests) struct NativeThreeCheckpointFixture {
    pub(in crate::egl_renderer::tests) output_size: (u32, u32),
    pub(in crate::egl_renderer::tests) output_bounds: EffectRect,
    pub(in crate::egl_renderer::tests) repair: OutputRect,
    pub(in crate::egl_renderer::tests) effect_a: NativeStackedBackdropEffectSpec,
    pub(in crate::egl_renderer::tests) effect_c: NativeStackedBackdropEffectSpec,
    pub(in crate::egl_renderer::tests) effect_b: NativeStackedBackdropEffectSpec,
    pub(in crate::egl_renderer::tests) scene: NativeThreeCheckpointSceneSpec,
    pub(in crate::egl_renderer::tests) same_anchor: bool,
}

pub(in crate::egl_renderer::tests) fn native_three_checkpoint_fixture()
-> NativeThreeCheckpointFixture {
    NativeThreeCheckpointFixture {
        output_size: (160, 120),
        output_bounds: EffectRect::new(0, 0, 160, 120).expect("three-checkpoint output bounds"),
        repair: OutputRect::new(76, 56, 4, 4),
        effect_a: NativeStackedBackdropEffectSpec {
            id: 31,
            target_bounds: EffectRect::new(20, 16, 96, 72).expect("three-checkpoint A bounds"),
            anchor: oblivion_one::compositor::EffectAnchor::BeforeSurface(101),
            anchor_scope: oblivion_one::compositor::EffectAnchorScope::Surface,
            visual_group: Some(VisualGroupId::new(101).expect("three-checkpoint A group")),
            scene_order: oblivion_one::compositor::EffectSceneOrder {
                group_order: 1,
                surface_order: 0,
                phase: 0,
            },
        },
        effect_c: NativeStackedBackdropEffectSpec {
            id: 32,
            target_bounds: EffectRect::new(32, 24, 96, 72).expect("three-checkpoint C bounds"),
            anchor: oblivion_one::compositor::EffectAnchor::BeforeSurface(102),
            anchor_scope: oblivion_one::compositor::EffectAnchorScope::Surface,
            visual_group: Some(VisualGroupId::new(102).expect("three-checkpoint C group")),
            scene_order: oblivion_one::compositor::EffectSceneOrder {
                group_order: 2,
                surface_order: 0,
                phase: 0,
            },
        },
        effect_b: NativeStackedBackdropEffectSpec {
            id: 33,
            target_bounds: EffectRect::new(44, 32, 96, 72).expect("three-checkpoint B bounds"),
            anchor: oblivion_one::compositor::EffectAnchor::BeforeSurface(103),
            anchor_scope: oblivion_one::compositor::EffectAnchorScope::Surface,
            visual_group: Some(VisualGroupId::new(103).expect("three-checkpoint B group")),
            scene_order: oblivion_one::compositor::EffectSceneOrder {
                group_order: 3,
                surface_order: 0,
                phase: 0,
            },
        },
        scene: NativeThreeCheckpointSceneSpec {
            background_surface: 100,
            a_surface: 101,
            c_surface: 102,
            b_surface: 103,
            a_content_bounds: EffectRect::new(8, 8, 80, 56)
                .expect("three-checkpoint A content bounds"),
            c_content_bounds: EffectRect::new(36, 24, 80, 56)
                .expect("three-checkpoint C content bounds"),
            b_content_bounds: EffectRect::new(64, 48, 80, 56)
                .expect("three-checkpoint B content bounds"),
            a_visual_group: VisualGroupId::new(101).expect("three-checkpoint A content group"),
            c_visual_group: VisualGroupId::new(102).expect("three-checkpoint C content group"),
            b_visual_group: VisualGroupId::new(103).expect("three-checkpoint B content group"),
        },
        same_anchor: false,
    }
}

pub(in crate::egl_renderer::tests) fn native_same_anchor_fixture() -> NativeThreeCheckpointFixture {
    let mut fixture = native_three_checkpoint_fixture();
    fixture.effect_c.anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(103);
    fixture.effect_b.anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(103);
    fixture.same_anchor = true;
    fixture
}

pub(in crate::egl_renderer::tests) fn native_three_checkpoint_backdrop_scene(
    fixture: NativeThreeCheckpointFixture,
) -> (ResolvedEffectScene, EffectRegistry) {
    let (program_scene, registry) =
        moving_blur_scene_with_spec(fixture.effect_a.target_bounds, 4.0, 2, 1.0);
    let program = program_scene
        .instances
        .first()
        .expect("three-checkpoint program instance")
        .program;
    let instances = [
        native_backdrop_effect_instance(fixture.effect_a, program),
        native_backdrop_effect_instance(fixture.effect_c, program),
        native_backdrop_effect_instance(fixture.effect_b, program),
    ];
    (ResolvedEffectScene::new(1, instances.to_vec()), registry)
}

pub(in crate::egl_renderer::tests) fn compile_native_three_checkpoint_graph(
    fixture: NativeThreeCheckpointFixture,
    source_damage: &EffectRegion,
) -> oblivion_one::effects::CompiledFrameGraph {
    let (scene, registry) = native_three_checkpoint_backdrop_scene(fixture);
    let plan = oblivion_one::effects::compile_frame_execution_plan(
        &scene,
        source_damage,
        fixture.output_bounds,
        &registry,
    )
    .expect("three-checkpoint diagnostic graph compiles");
    let oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) = plan else {
        panic!("three-checkpoint diagnostic graph must be an effect graph");
    };
    graph
}

pub(in crate::egl_renderer::tests) fn install_visual_group_scene_commands(
    renderer: &mut GlesSceneRenderer,
    rect: EffectRect,
    visual_group: VisualGroupId,
) {
    const BACKGROUND_COLOR: u32 = 0xff20_4060;
    renderer.scene_state.vertices.clear();
    renderer.scene_state.commands.clear();
    push_draw_command(
        &mut renderer.scene_state.vertices,
        &mut renderer.scene_state.commands,
        EglDrawLayer::SolidRgba(BACKGROUND_COLOR),
        EglRect::new(
            0.0,
            0.0,
            renderer.scene_state.current_size.0 as f32,
            renderer.scene_state.current_size.1 as f32,
        ),
        renderer.scene_state.current_size.0,
        renderer.scene_state.current_size.1,
        OutputFramebufferOrigin::BottomLeft,
    );
    let target_command = renderer.scene_state.commands.len();
    push_draw_command(
        &mut renderer.scene_state.vertices,
        &mut renderer.scene_state.commands,
        EglDrawLayer::Surface(42),
        EglRect::new(
            rect.x as f32,
            rect.y as f32,
            rect.width as f32,
            rect.height as f32,
        ),
        renderer.scene_state.current_size.0,
        renderer.scene_state.current_size.1,
        OutputFramebufferOrigin::BottomLeft,
    );
    renderer.scene_state.commands[target_command].visual_group = Some(visual_group);
    renderer.scene_state.scene_geometry_dirty = true;
}

pub(in crate::egl_renderer::tests) fn install_diagnostic_scene(
    harness: &mut GlesEffectTestHarness,
    rect: EffectRect,
    visual_group: VisualGroupId,
) {
    let width = harness.renderer.scene_state.current_size.0;
    let height = harness.renderer.scene_state.current_size.1;
    let mut background = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            background.extend_from_slice(&[
                24_u8.saturating_add((x * 3 % 180) as u8),
                18_u8.saturating_add((y * 5 % 180) as u8),
                32_u8.saturating_add(((x + y) * 2 % 160) as u8),
                255,
            ]);
        }
    }
    let background_image = create_uploaded_resource(&harness.gl, width, height)
        .expect("diagnostic background texture creates");
    write_rgba_bytes_to_resource(
        &harness.gl,
        &background_image,
        SurfaceDamageRect::full(width, height),
        &background,
    );
    harness.renderer.surface_resources.insert(
        7,
        EglSurfaceResource {
            image: background_image,
            dmabuf_key: None,
            buffer_lifetime: None,
            shm_synced_commit: None,
        },
    );
    let translucent_surface = [
        112_u8, 18, 12, 128, 18, 112, 12, 160, 18, 12, 112, 96, 112, 112, 12, 192,
    ];
    let target_image = create_uploaded_resource(&harness.gl, 2, 2)
        .expect("diagnostic translucent surface texture creates");
    write_rgba_bytes_to_resource(
        &harness.gl,
        &target_image,
        SurfaceDamageRect::full(2, 2),
        &translucent_surface,
    );
    harness.renderer.surface_resources.insert(
        42,
        EglSurfaceResource {
            image: target_image,
            dmabuf_key: None,
            buffer_lifetime: None,
            shm_synced_commit: None,
        },
    );
    harness.renderer.scene_state.vertices.clear();
    harness.renderer.scene_state.commands.clear();
    push_draw_command(
        &mut harness.renderer.scene_state.vertices,
        &mut harness.renderer.scene_state.commands,
        EglDrawLayer::Surface(7),
        EglRect::new(0.0, 0.0, width as f32, height as f32),
        width,
        height,
        OutputFramebufferOrigin::BottomLeft,
    );
    let target_command = harness.renderer.scene_state.commands.len();
    push_draw_command(
        &mut harness.renderer.scene_state.vertices,
        &mut harness.renderer.scene_state.commands,
        EglDrawLayer::Surface(42),
        EglRect::new(
            rect.x as f32,
            rect.y as f32,
            rect.width as f32,
            rect.height as f32,
        ),
        width,
        height,
        OutputFramebufferOrigin::BottomLeft,
    );
    harness.renderer.scene_state.commands[target_command].visual_group = Some(visual_group);
    harness.renderer.scene_state.scene_geometry_dirty = true;
}

#[derive(Clone, Copy)]
pub(in crate::egl_renderer::tests) struct NativeStackedBackdropSceneSpec {
    pub(in crate::egl_renderer::tests) background_surface: u32,
    pub(in crate::egl_renderer::tests) a_surface: u32,
    pub(in crate::egl_renderer::tests) b_surface: u32,
    pub(in crate::egl_renderer::tests) a_content_bounds: EffectRect,
    pub(in crate::egl_renderer::tests) b_content_bounds: EffectRect,
    pub(in crate::egl_renderer::tests) a_visual_group: VisualGroupId,
    pub(in crate::egl_renderer::tests) b_visual_group: VisualGroupId,
}

pub(in crate::egl_renderer::tests) fn insert_native_test_surface(
    harness: &mut GlesEffectTestHarness,
    surface_id: u32,
    width: u32,
    height: u32,
    pixels: &[u8],
) {
    let image = create_uploaded_resource(&harness.gl, width, height)
        .expect("native diagnostic surface texture creates");
    write_rgba_bytes_to_resource(
        &harness.gl,
        &image,
        SurfaceDamageRect::full(width, height),
        pixels,
    );
    harness.renderer.surface_resources.insert(
        surface_id,
        EglSurfaceResource {
            image,
            dmabuf_key: None,
            buffer_lifetime: None,
            shm_synced_commit: None,
        },
    );
}

pub(in crate::egl_renderer::tests) fn install_native_stacked_diagnostic_scene(
    harness: &mut GlesEffectTestHarness,
    spec: NativeStackedBackdropSceneSpec,
) {
    let width = harness.renderer.scene_state.current_size.0;
    let height = harness.renderer.scene_state.current_size.1;
    let mut background = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            background.extend_from_slice(&[
                24_u8.saturating_add((x * 3 % 180) as u8),
                18_u8.saturating_add((y * 5 % 180) as u8),
                32_u8.saturating_add(((x + y) * 2 % 160) as u8),
                255,
            ]);
        }
    }
    let background_image = create_uploaded_resource(&harness.gl, width, height)
        .expect("native diagnostic background texture creates");
    write_rgba_bytes_to_resource(
        &harness.gl,
        &background_image,
        SurfaceDamageRect::full(width, height),
        &background,
    );
    harness.renderer.surface_resources.insert(
        spec.background_surface,
        EglSurfaceResource {
            image: background_image,
            dmabuf_key: None,
            buffer_lifetime: None,
            shm_synced_commit: None,
        },
    );

    let a_surface = [
        112_u8, 18, 12, 128, 18, 112, 12, 160, 18, 12, 112, 96, 112, 112, 12, 192,
    ];
    let a_image =
        create_uploaded_resource(&harness.gl, 2, 2).expect("native diagnostic A texture creates");
    write_rgba_bytes_to_resource(
        &harness.gl,
        &a_image,
        SurfaceDamageRect::full(2, 2),
        &a_surface,
    );
    harness.renderer.surface_resources.insert(
        spec.a_surface,
        EglSurfaceResource {
            image: a_image,
            dmabuf_key: None,
            buffer_lifetime: None,
            shm_synced_commit: None,
        },
    );

    let b_surface = [
        12_u8, 18, 112, 144, 112, 18, 12, 176, 12, 112, 18, 112, 88, 88, 112, 192,
    ];
    let b_image =
        create_uploaded_resource(&harness.gl, 2, 2).expect("native diagnostic B texture creates");
    write_rgba_bytes_to_resource(
        &harness.gl,
        &b_image,
        SurfaceDamageRect::full(2, 2),
        &b_surface,
    );
    harness.renderer.surface_resources.insert(
        spec.b_surface,
        EglSurfaceResource {
            image: b_image,
            dmabuf_key: None,
            buffer_lifetime: None,
            shm_synced_commit: None,
        },
    );

    harness.renderer.scene_state.vertices.clear();
    harness.renderer.scene_state.commands.clear();
    push_draw_command(
        &mut harness.renderer.scene_state.vertices,
        &mut harness.renderer.scene_state.commands,
        EglDrawLayer::Surface(spec.background_surface),
        EglRect::new(0.0, 0.0, width as f32, height as f32),
        width,
        height,
        OutputFramebufferOrigin::BottomLeft,
    );
    let a_command = harness.renderer.scene_state.commands.len();
    push_draw_command(
        &mut harness.renderer.scene_state.vertices,
        &mut harness.renderer.scene_state.commands,
        EglDrawLayer::Surface(spec.a_surface),
        EglRect::new(
            spec.a_content_bounds.x as f32,
            spec.a_content_bounds.y as f32,
            spec.a_content_bounds.width as f32,
            spec.a_content_bounds.height as f32,
        ),
        width,
        height,
        OutputFramebufferOrigin::BottomLeft,
    );
    harness.renderer.scene_state.commands[a_command].visual_group = Some(spec.a_visual_group);
    let b_command = harness.renderer.scene_state.commands.len();
    push_draw_command(
        &mut harness.renderer.scene_state.vertices,
        &mut harness.renderer.scene_state.commands,
        EglDrawLayer::Surface(spec.b_surface),
        EglRect::new(
            spec.b_content_bounds.x as f32,
            spec.b_content_bounds.y as f32,
            spec.b_content_bounds.width as f32,
            spec.b_content_bounds.height as f32,
        ),
        width,
        height,
        OutputFramebufferOrigin::BottomLeft,
    );
    harness.renderer.scene_state.commands[b_command].visual_group = Some(spec.b_visual_group);
    harness.renderer.scene_state.scene_geometry_dirty = true;
}

pub(in crate::egl_renderer::tests) fn install_native_three_checkpoint_diagnostic_scene(
    harness: &mut GlesEffectTestHarness,
    spec: NativeThreeCheckpointSceneSpec,
) {
    let width = harness.renderer.scene_state.current_size.0;
    let height = harness.renderer.scene_state.current_size.1;
    let mut background = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            background.extend_from_slice(&[
                24_u8.saturating_add((x * 3 % 180) as u8),
                18_u8.saturating_add((y * 5 % 180) as u8),
                32_u8.saturating_add(((x + y) * 2 % 160) as u8),
                255,
            ]);
        }
    }
    insert_native_test_surface(harness, spec.background_surface, width, height, &background);
    insert_native_test_surface(
        harness,
        spec.a_surface,
        2,
        2,
        &[
            112_u8, 18, 12, 128, 18, 112, 12, 160, 18, 12, 112, 96, 112, 112, 12, 192,
        ],
    );
    insert_native_test_surface(
        harness,
        spec.c_surface,
        2,
        2,
        &[
            12_u8, 112, 18, 144, 88, 18, 112, 176, 18, 88, 112, 112, 112, 18, 88, 192,
        ],
    );
    insert_native_test_surface(
        harness,
        spec.b_surface,
        2,
        2,
        &[
            12_u8, 18, 112, 144, 112, 18, 12, 176, 12, 112, 18, 112, 88, 88, 112, 192,
        ],
    );

    harness.renderer.scene_state.vertices.clear();
    harness.renderer.scene_state.commands.clear();
    push_draw_command(
        &mut harness.renderer.scene_state.vertices,
        &mut harness.renderer.scene_state.commands,
        EglDrawLayer::Surface(spec.background_surface),
        EglRect::new(0.0, 0.0, width as f32, height as f32),
        width,
        height,
        OutputFramebufferOrigin::BottomLeft,
    );
    let a_command = harness.renderer.scene_state.commands.len();
    push_draw_command(
        &mut harness.renderer.scene_state.vertices,
        &mut harness.renderer.scene_state.commands,
        EglDrawLayer::Surface(spec.a_surface),
        EglRect::new(
            spec.a_content_bounds.x as f32,
            spec.a_content_bounds.y as f32,
            spec.a_content_bounds.width as f32,
            spec.a_content_bounds.height as f32,
        ),
        width,
        height,
        OutputFramebufferOrigin::BottomLeft,
    );
    harness.renderer.scene_state.commands[a_command].visual_group = Some(spec.a_visual_group);
    let c_command = harness.renderer.scene_state.commands.len();
    push_draw_command(
        &mut harness.renderer.scene_state.vertices,
        &mut harness.renderer.scene_state.commands,
        EglDrawLayer::Surface(spec.c_surface),
        EglRect::new(
            spec.c_content_bounds.x as f32,
            spec.c_content_bounds.y as f32,
            spec.c_content_bounds.width as f32,
            spec.c_content_bounds.height as f32,
        ),
        width,
        height,
        OutputFramebufferOrigin::BottomLeft,
    );
    harness.renderer.scene_state.commands[c_command].visual_group = Some(spec.c_visual_group);
    let b_command = harness.renderer.scene_state.commands.len();
    push_draw_command(
        &mut harness.renderer.scene_state.vertices,
        &mut harness.renderer.scene_state.commands,
        EglDrawLayer::Surface(spec.b_surface),
        EglRect::new(
            spec.b_content_bounds.x as f32,
            spec.b_content_bounds.y as f32,
            spec.b_content_bounds.width as f32,
            spec.b_content_bounds.height as f32,
        ),
        width,
        height,
        OutputFramebufferOrigin::BottomLeft,
    );
    harness.renderer.scene_state.commands[b_command].visual_group = Some(spec.b_visual_group);
    assert!(a_command < c_command && c_command < b_command);
    harness.renderer.scene_state.scene_geometry_dirty = true;
}

#[derive(Clone, Copy)]
pub(in crate::egl_renderer::tests) struct NativeStackedDiagnosticFixture {
    pub(in crate::egl_renderer::tests) output_size: (u32, u32),
    pub(in crate::egl_renderer::tests) output_bounds: EffectRect,
    pub(in crate::egl_renderer::tests) repair: OutputRect,
    pub(in crate::egl_renderer::tests) effect_a: NativeStackedBackdropEffectSpec,
    pub(in crate::egl_renderer::tests) effect_b: NativeStackedBackdropEffectSpec,
    pub(in crate::egl_renderer::tests) scene: NativeStackedBackdropSceneSpec,
}

pub(in crate::egl_renderer::tests) fn native_dock_fixture() -> NativeStackedDiagnosticFixture {
    NativeStackedDiagnosticFixture {
        output_size: (1920, 1080),
        output_bounds: EffectRect::new(0, 0, 1920, 1080).expect("native Dock output bounds"),
        repair: OutputRect::new(1100, 1010, 8, 8),
        effect_a: NativeStackedBackdropEffectSpec {
            id: 1,
            target_bounds: EffectRect::new(90, 144, 952, 889).expect("native Dock A bounds"),
            anchor: oblivion_one::compositor::EffectAnchor::BeforeSurface(11),
            anchor_scope: oblivion_one::compositor::EffectAnchorScope::VisualGroup,
            visual_group: Some(VisualGroupId::new(11).expect("native Dock A visual group")),
            scene_order: oblivion_one::compositor::EffectSceneOrder {
                group_order: 1,
                surface_order: 0,
                phase: 0,
            },
        },
        effect_b: NativeStackedBackdropEffectSpec {
            id: 2,
            target_bounds: EffectRect::new(786, 1000, 348, 56).expect("native Dock B bounds"),
            anchor: oblivion_one::compositor::EffectAnchor::BeforeSurface(7),
            anchor_scope: oblivion_one::compositor::EffectAnchorScope::Surface,
            visual_group: Some(VisualGroupId::new(7).expect("native Dock B visual group")),
            scene_order: oblivion_one::compositor::EffectSceneOrder {
                group_order: 2,
                surface_order: 0,
                phase: 0,
            },
        },
        scene: NativeStackedBackdropSceneSpec {
            background_surface: 100,
            a_surface: 11,
            b_surface: 7,
            a_content_bounds: EffectRect::new(650, 899, 416, 158)
                .expect("native Dock A content bounds"),
            b_content_bounds: EffectRect::new(786, 1000, 348, 56)
                .expect("native Dock B content bounds"),
            a_visual_group: VisualGroupId::new(11).expect("native Dock A content group"),
            b_visual_group: VisualGroupId::new(7).expect("native Dock B content group"),
        },
    }
}

pub(in crate::egl_renderer::tests) fn native_topbar_fixture() -> NativeStackedDiagnosticFixture {
    NativeStackedDiagnosticFixture {
        output_size: (1920, 1080),
        output_bounds: EffectRect::new(0, 0, 1920, 1080).expect("native TopBar output bounds"),
        repair: OutputRect::new(72, 24, 8, 8),
        effect_a: NativeStackedBackdropEffectSpec {
            id: 11,
            target_bounds: EffectRect::new(0, 0, 120, 65).expect("native TopBar A bounds"),
            anchor: oblivion_one::compositor::EffectAnchor::BeforeSurface(11),
            anchor_scope: oblivion_one::compositor::EffectAnchorScope::VisualGroup,
            visual_group: Some(VisualGroupId::new(11).expect("native TopBar A group")),
            scene_order: oblivion_one::compositor::EffectSceneOrder {
                group_order: 1,
                surface_order: 0,
                phase: 0,
            },
        },
        effect_b: NativeStackedBackdropEffectSpec {
            id: 12,
            target_bounds: EffectRect::new(24, 24, 72, 17).expect("native TopBar B bounds"),
            anchor: oblivion_one::compositor::EffectAnchor::BeforeSurface(13),
            anchor_scope: oblivion_one::compositor::EffectAnchorScope::Surface,
            visual_group: Some(VisualGroupId::new(13).expect("native TopBar B group")),
            scene_order: oblivion_one::compositor::EffectSceneOrder {
                group_order: 2,
                surface_order: 0,
                phase: 0,
            },
        },
        scene: NativeStackedBackdropSceneSpec {
            background_surface: 100,
            a_surface: 11,
            b_surface: 13,
            a_content_bounds: EffectRect::new(0, 0, 120, 65)
                .expect("native TopBar A content bounds"),
            b_content_bounds: EffectRect::new(24, 24, 72, 17)
                .expect("native TopBar B content bounds"),
            a_visual_group: VisualGroupId::new(11).expect("native TopBar A content group"),
            b_visual_group: VisualGroupId::new(13).expect("native TopBar B content group"),
        },
    }
}

pub(in crate::egl_renderer::tests) fn compile_native_stacked_graph(
    fixture: NativeStackedDiagnosticFixture,
    source_damage: &EffectRegion,
) -> oblivion_one::effects::CompiledFrameGraph {
    let (scene, registry) =
        native_stacked_backdrop_scene(fixture.effect_a, fixture.effect_b, 4.0, 2, 1.0);
    let plan = oblivion_one::effects::compile_frame_execution_plan(
        &scene,
        source_damage,
        fixture.output_bounds,
        &registry,
    )
    .expect("native stacked diagnostic graph compiles");
    let oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) = plan else {
        panic!("native stacked diagnostic graph must be an effect graph");
    };
    graph
}

pub(in crate::egl_renderer::tests) fn diagnostic_graph(
    rect: EffectRect,
    visual_group: VisualGroupId,
    source_damage: &EffectRegion,
    output_bounds: EffectRect,
) -> oblivion_one::effects::CompiledFrameGraph {
    diagnostic_graph_with_spec(
        rect,
        visual_group,
        source_damage,
        output_bounds,
        4.0,
        2,
        1.0,
    )
}

pub(in crate::egl_renderer::tests) fn diagnostic_graph_with_spec(
    rect: EffectRect,
    visual_group: VisualGroupId,
    source_damage: &EffectRegion,
    output_bounds: EffectRect,
    radius: f32,
    passes: u8,
    scale: f32,
) -> oblivion_one::effects::CompiledFrameGraph {
    let scene = moving_visual_group_blur_scene_with_spec(rect, visual_group, radius, passes, scale);
    let (_, registry) = moving_blur_scene_with_spec(rect, radius, passes, scale);
    let plan = oblivion_one::effects::compile_frame_execution_plan(
        &scene,
        source_damage,
        output_bounds,
        &registry,
    )
    .expect("diagnostic blur graph compiles");
    let oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) = plan else {
        panic!("diagnostic blur must compile to an effect graph");
    };
    graph
}

pub(in crate::egl_renderer::tests) fn diagnostic_graph_with_linear_neighbor_stage(
    rect: EffectRect,
    visual_group: VisualGroupId,
    source_damage: &EffectRegion,
    output_bounds: EffectRect,
) -> oblivion_one::effects::CompiledFrameGraph {
    let source = EffectNodeId::new(1).expect("custom diagnostic source id");
    let blur = EffectNodeId::new(2).expect("custom diagnostic blur id");
    let custom = EffectNodeId::new(3).expect("custom diagnostic stage id");
    let program_id = EffectProgramId::new(2).expect("custom diagnostic program id");
    let custom_shader = ShaderModuleId::new(9_001).expect("custom diagnostic shader id");
    let program = validate_effect_program(EffectProgram {
        id: program_id,
        nodes: vec![
            EffectNode::source(source, EffectSource::Backdrop),
            EffectNode::dual_kawase(
                blur,
                source,
                DualKawaseBlurSpec::new(4.0, 2, 0.5).expect("custom diagnostic blur spec"),
            ),
            EffectNode::custom_fragment(
                custom,
                blur,
                CustomFragmentSpec {
                    shader: custom_shader,
                    declared_footprint: EffectFootprint::symmetric(1),
                    uniforms: Vec::new(),
                    auxiliary_inputs: Vec::new(),
                },
            )
            .expect("custom diagnostic stage validates"),
        ],
        output: custom,
        working_space: EffectWorkingSpace::LinearSrgb,
        alpha_mode: EffectAlphaMode::Opaque,
        outsets: oblivion_one::effects::EffectOutsets::ZERO,
        frame_demand: EffectFrameDemand::OnDamage,
        failure_policy: EffectFailurePolicy::Passthrough,
    })
    .expect("custom diagnostic program validates");
    let mut registry = EffectRegistry::empty();
    registry
        .insert(program)
        .expect("custom diagnostic program inserts");
    let mut scene = moving_visual_group_blur_scene_with_spec(rect, visual_group, 4.0, 2, 0.5);
    scene.instances[0].program = program_id;
    let plan = oblivion_one::effects::compile_frame_execution_plan(
        &scene,
        source_damage,
        output_bounds,
        &registry,
    )
    .expect("custom diagnostic graph compiles");
    let oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) = plan else {
        panic!("custom diagnostic graph must compile to an effect graph");
    };
    graph
}

pub(in crate::egl_renderer::tests) fn install_custom_linear_neighbor_shader(
    harness: &mut GlesEffectTestHarness,
) {
    let custom_shader = ShaderModuleId::new(9_001).expect("custom diagnostic shader id");
    let shader_key =
        effects::ShaderProgramKey::new(custom_shader, 0, EffectWorkingSpace::LinearSrgb);
    let source = effects::generate_fragment_wrapper(
        r#"vec4 typhon_effect_main(TyphonEffectContext ctx) {
            vec2 delta = vec2(0.75) / ctx.texture_size;
            return (typhon_sample_primary(ctx.uv + delta) +
                typhon_sample_primary(ctx.uv - delta)) * 0.5;
        }"#,
        &[],
    )
    .expect("custom diagnostic shader wraps");
    harness.renderer.effect_runtime.effect_shaders =
        effects::ShaderProgramCache::new(effects::builtin_shader_program_count() + 1)
            .expect("custom diagnostic shader cache creates");
    harness
        .renderer
        .effect_runtime
        .effect_shaders
        .prewarm_builtins(&harness.gl)
        .expect("custom diagnostic builtins prewarm");
    harness
        .renderer
        .effect_runtime
        .effect_shaders
        .prewarm(
            &harness.gl,
            shader_key,
            effects::DUAL_KAWASE_VERTEX_SHADER,
            &source,
        )
        .expect("custom diagnostic shader prewarms");
}

pub(in crate::egl_renderer::tests) fn diagnostic_repaint_plan(
    repair: OutputRect,
    full: bool,
) -> RepaintPlan {
    diagnostic_repaint_plan_for_repairs(&[repair], full)
}

pub(in crate::egl_renderer::tests) fn diagnostic_repaint_plan_for_repairs(
    repairs: &[OutputRect],
    full: bool,
) -> RepaintPlan {
    diagnostic_repaint_plan_for_repairs_in_size(repairs, full, (128, 96))
}

pub(in crate::egl_renderer::tests) fn diagnostic_repaint_plan_for_repairs_in_size(
    repairs: &[OutputRect],
    full: bool,
    output_size: (u32, u32),
) -> RepaintPlan {
    let damage = if full {
        OutputDamage::Full
    } else {
        OutputDamage::rects(output_size.0, output_size.1, repairs.iter().copied())
    };
    RepaintPlan {
        render_damage: damage.clone(),
        repair_damage: damage,
        buffer_age: (!full).then_some(2),
        mode: if full {
            RepaintMode::Full
        } else {
            RepaintMode::Partial
        },
        fallback_reason: None,
        ..RepaintPlan::default()
    }
}

pub(in crate::egl_renderer::tests) fn read_diagnostic_pixels(
    harness: &GlesEffectTestHarness,
) -> Vec<u8> {
    let width = harness.renderer.scene_state.current_size.0;
    let height = harness.renderer.scene_state.current_size.1;
    let mut pixels = vec![0_u8; width as usize * height as usize * 4];
    unsafe {
        harness.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
        harness.gl.read_pixels(
            0,
            0,
            width as i32,
            height as i32,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut pixels)),
        );
    }
    pixels
}

pub(in crate::egl_renderer::tests) fn update_diagnostic_background_for_surface(
    harness: &GlesEffectTestHarness,
    surface_id: u32,
    repair: OutputRect,
    color: [u8; 4],
) {
    let resource = &harness
        .renderer
        .surface_resources
        .get(&surface_id)
        .expect("diagnostic background resource")
        .image;
    let pixels = vec![color; repair.width as usize * repair.height as usize]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    write_rgba_bytes_to_resource(
        &harness.gl,
        resource,
        SurfaceDamageRect {
            x: repair.x as u32,
            y: repair.y as u32,
            width: repair.width,
            height: repair.height,
        },
        &pixels,
    );
}

pub(in crate::egl_renderer::tests) fn poison_diagnostic_output(
    harness: &GlesEffectTestHarness,
    color: [f32; 4],
) {
    harness.renderer.bind_active_output_framebuffer();
    unsafe {
        harness.gl.disable(glow::SCISSOR_TEST);
        harness.gl.disable(glow::BLEND);
        harness
            .gl
            .clear_color(color[0], color[1], color[2], color[3]);
        harness.gl.clear(glow::COLOR_BUFFER_BIT);
    }
    harness.renderer.establish_ordinary_scene_state();
}

pub(in crate::egl_renderer::tests) fn fill_non_uniform_diagnostic_output(
    harness: &GlesEffectTestHarness,
) {
    let width = harness.renderer.scene_state.current_size.0;
    let height = harness.renderer.scene_state.current_size.1;
    harness.renderer.bind_active_output_framebuffer();
    unsafe {
        harness.gl.disable(glow::BLEND);
        harness.gl.enable(glow::SCISSOR_TEST);
        for y in 0..height {
            for x in 0..width {
                harness.gl.scissor(x as i32, y as i32, 1, 1);
                harness.gl.clear_color(
                    f32::from(((x * 31 + y * 17 + 3) % 256) as u8) / 255.0,
                    f32::from(((x * 13 + y * 29 + 7) % 256) as u8) / 255.0,
                    f32::from(((x * 47 + y * 11 + 19) % 256) as u8) / 255.0,
                    1.0,
                );
                harness.gl.clear(glow::COLOR_BUFFER_BIT);
            }
        }
        harness.gl.disable(glow::SCISSOR_TEST);
    }
    harness.renderer.establish_ordinary_scene_state();
}

pub(in crate::egl_renderer::tests) fn fill_shader_copy_test_pattern(
    harness: &GlesEffectTestHarness,
) {
    let width = harness.renderer.scene_state.current_size.0;
    let height = harness.renderer.scene_state.current_size.1;
    harness.renderer.bind_active_output_framebuffer();
    unsafe {
        harness.gl.disable(glow::BLEND);
        harness.gl.enable(glow::SCISSOR_TEST);
        for y in 0..height {
            for x in 0..width {
                harness.gl.scissor(x as i32, y as i32, 1, 1);
                let checker = ((x / 3 + y / 3) % 2) as u8;
                harness.gl.clear_color(
                    f32::from((((x * 31 + y * 17 + 3) % 256) as u8) ^ (checker * 0x3f)) / 255.0,
                    f32::from(((x * 13 + y * 29 + 7) % 256) as u8) / 255.0,
                    f32::from(((x * 47 + y * 11 + 19) % 256) as u8) / 255.0,
                    f32::from(((x * 19 + y * 23 + 61) % 256) as u8) / 255.0,
                );
                harness.gl.clear(glow::COLOR_BUFFER_BIT);
            }
        }
        harness.gl.disable(glow::SCISSOR_TEST);
    }
    harness.renderer.establish_ordinary_scene_state();
}

pub(in crate::egl_renderer::tests) fn lifecycle_transfer_test_color(
    x: u32,
    logical_y: u32,
) -> [u8; 4] {
    match logical_y {
        0..=1 => [255, (x * 19) as u8, 0, 255],
        2..=3 => [0, 255, (x * 23) as u8, 255],
        _ => [0, (x * 17) as u8, 255, 255],
    }
}

pub(in crate::egl_renderer::tests) fn fill_lifecycle_transfer_test_pattern(
    harness: &GlesEffectTestHarness,
    framebuffer_origin: OutputFramebufferOrigin,
) {
    let (width, height) = harness.renderer.scene_state.current_size;
    harness.renderer.bind_active_output_framebuffer();
    unsafe {
        harness.gl.disable(glow::BLEND);
        harness.gl.enable(glow::SCISSOR_TEST);
        for logical_y in 0..height {
            let gl_y = match framebuffer_origin {
                OutputFramebufferOrigin::BottomLeft => height - logical_y - 1,
                OutputFramebufferOrigin::TopLeftScanout => logical_y,
            };
            for x in 0..width {
                let [red, green, blue, alpha] = lifecycle_transfer_test_color(x, logical_y);
                harness.gl.scissor(x as i32, gl_y as i32, 1, 1);
                harness.gl.clear_color(
                    f32::from(red) / 255.0,
                    f32::from(green) / 255.0,
                    f32::from(blue) / 255.0,
                    f32::from(alpha) / 255.0,
                );
                harness.gl.clear(glow::COLOR_BUFFER_BIT);
            }
        }
        harness.gl.disable(glow::SCISSOR_TEST);
    }
    harness.renderer.establish_ordinary_scene_state();
}

pub(in crate::egl_renderer::tests) fn lifecycle_transfer_test_texture_key(
    width: u32,
    height: u32,
) -> EffectTextureKey {
    EffectTextureKey::new(
        width,
        height,
        EffectTextureFormat::Rgba8,
        EffectTextureFilter::Nearest,
        EffectWorkingSpace::OutputEncodedSrgb,
    )
}

pub(in crate::egl_renderer::tests) fn read_effect_texture_pixels(
    harness: &mut GlesEffectTestHarness,
    target: &PooledEffectTexture,
    width: u32,
    height: u32,
) -> Vec<u8> {
    harness
        .renderer
        .effect_runtime
        .effect_resources
        .bind_read_target(&harness.gl, target)
        .expect("capture target binds for readback");
    let mut pixels = vec![0_u8; width as usize * height as usize * 4];
    unsafe {
        harness.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
        harness.gl.read_pixels(
            0,
            0,
            width as i32,
            height as i32,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut pixels)),
        );
    }
    harness.renderer.establish_ordinary_scene_state();
    pixels
}
