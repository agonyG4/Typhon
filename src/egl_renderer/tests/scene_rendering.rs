use super::*;

#[test]
fn lamp_renderer_locates_continuous_motion_uniforms() {
    let harness = GlesEffectTestHarness::new(320, 200);
    assert_eq!(
        harness.renderer.lifecycle.lamp_uniforms_present_for_test(),
        [true; 7],
        "the lifecycle owner initializes all Lamp uniforms"
    );
}

#[test]
fn structured_partial_repaint_updates_every_repair_scissor_only() {
    let mut harness = GlesEffectTestHarness::new(8, 6);
    harness.install_texture_backed_output();
    let rects = [
        OutputRect::new(0, 0, 1, 1),
        OutputRect::new(3, 0, 1, 1),
        OutputRect::new(6, 0, 1, 1),
        OutputRect::new(0, 2, 1, 1),
        OutputRect::new(3, 2, 1, 1),
        OutputRect::new(6, 2, 1, 1),
        OutputRect::new(0, 4, 1, 1),
        OutputRect::new(3, 4, 1, 1),
        OutputRect::new(6, 4, 1, 1),
    ];
    let repair_damage = OutputDamage::rects(8, 6, rects);
    assert_eq!(repair_damage.rect_count(), 9);
    unsafe {
        harness.gl.disable(glow::SCISSOR_TEST);
        harness.gl.clear_color(1.0, 0.0, 0.0, 1.0);
        harness.gl.clear(glow::COLOR_BUFFER_BIT);
    }
    let plan = RepaintPlan {
        render_damage: repair_damage.clone(),
        repair_damage,
        buffer_age: Some(1),
        mode: RepaintMode::Partial,
        fallback_reason: None,
        complexity_policy: PartialRepaintComplexityPolicy::StructuredExperimental,
        complexity_action: PartialRepaintComplexityAction::StructuredManyRectangles,
    };

    let executed_rects = harness
        .renderer
        .begin_effect_repaint(&plan, OutputFramebufferOrigin::BottomLeft)
        .expect("structured partial repaint clears each repair rect");
    let pixels = read_effect_test_pixels(&harness.gl, 8, 6);

    assert_eq!(executed_rects.len(), 9);
    for gl_y in 0..6 {
        let logical_y = 5 - gl_y;
        for x in 0..8 {
            let repaired = rects.iter().any(|rect| {
                x >= rect.x as u32
                    && x < rect.x as u32 + rect.width
                    && logical_y >= rect.y as u32
                    && logical_y < rect.y as u32 + rect.height
            });
            assert_effect_test_pixel(
                &pixels,
                8,
                x,
                gl_y,
                if repaired {
                    [0, 0, 0, 255]
                } else {
                    [255, 0, 0, 255]
                },
            );
        }
    }
}

mod checkpoint_causal_snapshot_tests {
    use super::*;

    fn surface_signature(surface_id: u32, generation: u64) -> EglSceneSurfaceSignature {
        EglSceneSurfaceSignature {
            surface_id,
            commit_sequence: generation,
            buffer_id: u64::from(surface_id) * 100 + generation,
            buffer_width: 64,
            buffer_height: 64,
            buffer_scale: 1,
            buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
            x: 0,
            y: 0,
            width: 64,
            height: 64,
            render_x: 0,
            render_y: 0,
            clip_x: 0,
            clip_y: 0,
            clip_width: 64,
            clip_height: 64,
            generation,
        }
    }

    fn scene_commands() -> (Vec<EglDrawCommand>, Vec<EglTexturedVertex>) {
        let mut commands = Vec::new();
        let mut vertices = Vec::new();
        for index in 0..3_u32 {
            let vertex_start = vertices.len() as u32;
            vertices.extend([
                EglTexturedVertex {
                    position: [index as f32, 0.0],
                    uv: [0.0, 0.0],
                },
                EglTexturedVertex {
                    position: [index as f32 + 1.0, 0.0],
                    uv: [1.0, 0.0],
                },
                EglTexturedVertex {
                    position: [index as f32 + 1.0, 1.0],
                    uv: [1.0, 1.0],
                },
                EglTexturedVertex {
                    position: [index as f32, 1.0],
                    uv: [0.0, 1.0],
                },
            ]);
            commands.push(EglDrawCommand {
                layer: EglDrawLayer::Surface(index + 1),
                visual_group: Some(VisualGroupId::new(index + 1).unwrap()),
                bounds: EglRect::new(index as f32, 0.0, 1.0, 1.0),
                opaque_regions: Vec::new(),
                presentation_clip: None,
                vertex_start,
                vertex_count: 4,
                sampling: SurfaceSampling::ExactNearest,
            });
        }
        (commands, vertices)
    }

    fn snapshot(
        commands: &[EglDrawCommand],
        vertices: &[EglTexturedVertex],
        signatures: &[EglSceneSurfaceSignature],
        opacities: &HashMap<VisualGroupId, f32>,
    ) -> EglCheckpointSceneCausalSnapshot {
        snapshot_with_owners(commands, vertices, signatures, opacities, &HashMap::new())
    }

    fn snapshot_with_owners(
        commands: &[EglDrawCommand],
        vertices: &[EglTexturedVertex],
        signatures: &[EglSceneSurfaceSignature],
        opacities: &HashMap<VisualGroupId, f32>,
        owners: &HashMap<VisualGroupId, u32>,
    ) -> EglCheckpointSceneCausalSnapshot {
        let presentation_opacities = commands
            .iter()
            .map(|command| {
                command
                    .visual_group
                    .and_then(|group| opacities.get(&group).copied())
                    .unwrap_or(1.0)
            })
            .collect::<Vec<_>>();
        EglCheckpointSceneCausalSnapshot::new(
            (16, 16),
            commands,
            vertices,
            signatures,
            &presentation_opacities,
            owners,
        )
    }

    #[test]
    fn later_source_change_preserves_the_exact_earlier_command_prefix() {
        let (commands, vertices) = scene_commands();
        let previous_signatures = (1..=3)
            .map(|id| surface_signature(id, 1))
            .collect::<Vec<_>>();
        let current_signatures = vec![
            surface_signature(1, 1),
            surface_signature(2, 1),
            surface_signature(3, 2),
        ];
        let previous = snapshot(&commands, &vertices, &previous_signatures, &HashMap::new());
        let current = snapshot(&commands, &vertices, &current_signatures, &HashMap::new());

        assert_eq!(previous.unchanged_command_prefix_len(&current), 2);
    }

    #[test]
    fn same_geometry_with_new_surface_pixels_is_not_equal() {
        let (commands, vertices) = scene_commands();
        let previous_signatures = (1..=3)
            .map(|id| surface_signature(id, 1))
            .collect::<Vec<_>>();
        let changed_signatures = vec![
            surface_signature(1, 1),
            surface_signature(2, 2),
            surface_signature(3, 1),
        ];
        let previous = snapshot(&commands, &vertices, &previous_signatures, &HashMap::new());
        let current = snapshot(&commands, &vertices, &changed_signatures, &HashMap::new());

        assert_eq!(previous.unchanged_command_prefix_len(&current), 1);
    }

    #[test]
    fn geometry_sampling_opacity_clip_and_uv_changes_break_prefix_equality() {
        let (commands, vertices) = scene_commands();
        let signatures = (1..=3)
            .map(|id| surface_signature(id, 1))
            .collect::<Vec<_>>();
        let previous = snapshot(&commands, &vertices, &signatures, &HashMap::new());

        let mut changed_commands = commands.clone();
        changed_commands[1].bounds = EglRect::new(3.0, 0.0, 1.0, 1.0);
        assert_eq!(
            previous.unchanged_command_prefix_len(&snapshot(
                &changed_commands,
                &vertices,
                &signatures,
                &HashMap::new(),
            )),
            1
        );

        changed_commands.clone_from(&commands);
        changed_commands[1].presentation_clip = Some(EglRect::new(2.0, 0.0, 0.5, 1.0));
        changed_commands[1].opaque_regions = vec![EglRect::new(2.0, 0.0, 0.25, 1.0)];
        changed_commands[1].sampling = SurfaceSampling::ScaledLinear;
        assert_eq!(
            previous.unchanged_command_prefix_len(&snapshot(
                &changed_commands,
                &vertices,
                &signatures,
                &HashMap::new(),
            )),
            1
        );

        let mut changed_vertices = vertices.clone();
        changed_vertices[4].uv[0] = 0.25;
        assert_eq!(
            previous.unchanged_command_prefix_len(&snapshot(
                &commands,
                &changed_vertices,
                &signatures,
                &HashMap::new(),
            )),
            1
        );

        let mut changed_opacity = HashMap::new();
        changed_opacity.insert(VisualGroupId::new(2).unwrap(), 0.5);
        assert_eq!(
            previous.unchanged_command_prefix_len(&snapshot(
                &commands,
                &vertices,
                &signatures,
                &changed_opacity,
            )),
            1
        );
    }

    #[test]
    fn presentation_owner_changes_break_command_prefix_equality() {
        let (commands, vertices) = scene_commands();
        let signatures = (1..=3)
            .map(|id| surface_signature(id, 1))
            .collect::<Vec<_>>();
        let changed_group = VisualGroupId::new(2).unwrap();
        let previous_owners = HashMap::from([(changed_group, 50)]);
        let current_owners = HashMap::from([(changed_group, 51)]);
        let previous = snapshot_with_owners(
            &commands,
            &vertices,
            &signatures,
            &HashMap::new(),
            &previous_owners,
        );
        let current = snapshot_with_owners(
            &commands,
            &vertices,
            &signatures,
            &HashMap::new(),
            &current_owners,
        );

        assert_eq!(previous.unchanged_command_prefix_len(&current), 1);
    }

    #[test]
    fn unsupported_command_source_stops_the_common_prefix() {
        let (commands, vertices) = scene_commands();
        let signatures = (1..=3)
            .map(|id| surface_signature(id, 1))
            .collect::<Vec<_>>();
        let previous = snapshot(&commands, &vertices, &signatures, &HashMap::new());
        let mut current_commands = commands;
        current_commands[1].layer = EglDrawLayer::Cursor;
        let current = snapshot(&current_commands, &vertices, &signatures, &HashMap::new());

        assert_eq!(previous.unchanged_command_prefix_len(&current), 1);
    }
}

#[test]
fn ordinary_egl_presentation_opacity_resolves_owner_values() {
    let scene_node_id =
        oblivion_one::core::SceneNodeId::from_raw(1).expect("presentation scene node");
    for value in [1.0, 0.5, 0.0] {
        let entry = PresentationGroupOpacity::with_scene_node(
            scene_node_id,
            701,
            PresentationOpacity::new(value).expect("presentation opacity"),
            None,
        );
        assert_eq!(
            GlesSceneRenderer::presentation_opacity_for_root(&[entry], 701),
            value as f32
        );
        assert_eq!(
            GlesSceneRenderer::presentation_opacity_for_root(&[entry], 999),
            1.0
        );
    }
}

#[test]
fn translucent_egl_presentation_command_is_not_an_opaque_occluder() {
    let mut harness = GlesEffectTestHarness::new(320, 200);
    let mut surface = test_shm_surface(RenderableSurfaceDamage::full());
    surface.opaque_region = SurfaceOpaqueRegion::Full;
    let surfaces = vec![surface];
    let signatures = egl_scene_surface_signatures(&surfaces);
    let owner_root = 701;
    let scene_node_id =
        oblivion_one::core::SceneNodeId::from_raw(1).expect("presentation scene node");
    let opacity = PresentationGroupOpacity::with_scene_node(
        scene_node_id,
        owner_root,
        PresentationOpacity::new(0.5).expect("presentation opacity"),
        None,
    );

    harness.renderer.rebuild_scene_commands(
        320,
        200,
        &surfaces,
        &[],
        &[],
        1,
        1.0,
        1,
        &signatures,
        &[],
        0,
        &[opacity],
        &[],
        &HashMap::from([(7, owner_root)]),
        OutputFramebufferOrigin::BottomLeft,
    );

    let translucent_command = harness
        .renderer
        .scene_state
        .commands
        .last()
        .expect("surface command is emitted");
    assert_eq!(
        harness.renderer.scene_state.presentation_opacities,
        vec![0.5]
    );
    assert!(translucent_command.opaque_regions.is_empty());

    let opaque = PresentationGroupOpacity::with_scene_node(
        scene_node_id,
        owner_root,
        PresentationOpacity::new(1.0).expect("presentation opacity"),
        None,
    );
    harness.renderer.rebuild_scene_commands(
        320,
        200,
        &surfaces,
        &[],
        &[],
        2,
        1.0,
        1,
        &signatures,
        &[],
        0,
        &[opaque],
        &[],
        &HashMap::from([(7, owner_root)]),
        OutputFramebufferOrigin::BottomLeft,
    );

    let opaque_command = harness
        .renderer
        .scene_state
        .commands
        .last()
        .expect("surface command is emitted");
    assert_eq!(
        harness.renderer.scene_state.presentation_opacities,
        vec![1.0]
    );
    assert!(!opaque_command.opaque_regions.is_empty());

    let zero = PresentationGroupOpacity::with_scene_node(
        scene_node_id,
        owner_root,
        PresentationOpacity::new(0.0).expect("presentation opacity"),
        None,
    );
    harness.renderer.rebuild_scene_commands(
        320,
        200,
        &surfaces,
        &[],
        &[],
        3,
        1.0,
        1,
        &signatures,
        &[],
        0,
        &[zero],
        &[],
        &HashMap::from([(7, owner_root)]),
        OutputFramebufferOrigin::BottomLeft,
    );

    let zero_command = harness
        .renderer
        .scene_state
        .commands
        .last()
        .expect("surface command is emitted");
    assert_eq!(
        harness.renderer.scene_state.presentation_opacities,
        vec![0.0]
    );
    assert!(zero_command.opaque_regions.is_empty());
}

#[test]
fn capture_renderer_state_restores_active_output_texture() {
    let mut harness = GlesEffectTestHarness::new(8, 8);
    let framebuffer = unsafe {
        harness
            .gl
            .create_framebuffer()
            .expect("state test framebuffer creates")
    };
    let texture = unsafe {
        harness
            .gl
            .create_texture()
            .expect("state test texture creates")
    };
    harness.renderer.scene_state.active_output_framebuffer = Some(framebuffer);
    harness.renderer.scene_state.active_output_texture = Some(texture);

    let snapshot = CaptureRendererState::take(&harness.renderer);
    harness.renderer.scene_state.active_output_framebuffer = None;
    harness.renderer.scene_state.active_output_texture = None;
    snapshot.restore(&mut harness.renderer);

    assert_eq!(
        harness.renderer.scene_state.active_output_framebuffer,
        Some(framebuffer)
    );
    assert_eq!(
        harness.renderer.scene_state.active_output_texture,
        Some(texture)
    );

    unsafe {
        harness.gl.delete_framebuffer(framebuffer);
        harness.gl.delete_texture(texture);
    }
}
#[test]
fn output_background_uses_dedicated_solid_scene_layer() {
    let mut vertices = Vec::new();
    let mut commands = Vec::new();
    push_output_background_command(
        &mut vertices,
        &mut commands,
        1280,
        800,
        OutputFramebufferOrigin::BottomLeft,
    );

    assert_eq!(
        commands.first().map(|command| command.layer),
        Some(EglDrawLayer::Solid(
            compositor::ServerFrameColor::OutputBackground
        ))
    );
}

#[test]
fn output_damage_tracker_separates_scene_rebuild_from_authoritative_damage() {
    let mut tracker = EglOutputDamageTracker::default();
    tracker.damage_for_frame(
        1280,
        800,
        true,
        None,
        DesktopVisualState::wallpaper_only(),
        None,
    );
    tracker.commit_presented(EglOutputDamageTracker::candidate_state(
        1280,
        800,
        DesktopVisualState::wallpaper_only(),
        None,
        &oblivion_one::cursor_theme::CompositorCursorImage::builtin_fallback(),
    ));
    let precise = OutputDamage::rects(1280, 800, [OutputRect::new(10, 20, 30, 40)]);

    assert_eq!(
        tracker.damage_for_frame(
            1280,
            800,
            true,
            Some(precise.clone()),
            DesktopVisualState::wallpaper_only(),
            None,
        ),
        precise
    );
}

#[test]
fn scene_damage_authority_preserves_explicit_empty_damage() {
    assert_eq!(
        resolve_scene_damage_authority(true, true, OutputDamage::Empty),
        (OutputDamage::Empty, false)
    );
}

#[test]
fn scene_damage_authority_falls_back_when_damage_is_missing() {
    assert_eq!(
        resolve_scene_damage_authority(true, false, OutputDamage::Empty),
        (OutputDamage::Full, true)
    );
}

#[test]
fn output_damage_tracker_limits_cursor_motion_to_old_and_new_bounds() {
    let mut tracker = EglOutputDamageTracker::default();
    tracker.damage_for_frame(
        1280,
        800,
        true,
        None,
        DesktopVisualState::with_cursor(10, 10),
        None,
    );
    tracker.commit_presented(EglOutputDamageTracker::candidate_state(
        1280,
        800,
        DesktopVisualState::with_cursor(10, 10),
        None,
        &oblivion_one::cursor_theme::CompositorCursorImage::builtin_fallback(),
    ));
    let damage = tracker.damage_for_frame(
        1280,
        800,
        false,
        Some(OutputDamage::Empty),
        DesktopVisualState::with_cursor(20, 22),
        None,
    );

    assert_eq!(damage.rect_count(), 2);
    let cursor_image = oblivion_one::cursor_theme::CompositorCursorImage::builtin_fallback();
    assert_eq!(
        damage.pixels(1280, 800),
        Some(u64::from(cursor_image.width) * u64::from(cursor_image.height) * 2)
    );
}

#[test]
fn egl_damage_uses_hotspot_adjusted_bounds() {
    let image = std::sync::Arc::new(
        oblivion_one::cursor_theme::CompositorCursorImage::from_argb8888(
            vec![0xff00_0000; 4 * 3],
            4,
            3,
            2,
            1,
        )
        .unwrap(),
    );
    let mut tracker = EglOutputDamageTracker::with_cursor_image(image.clone());
    tracker.damage_for_frame(
        1280,
        800,
        true,
        None,
        DesktopVisualState::with_cursor(10, 10),
        None,
    );
    tracker.commit_presented(EglOutputDamageTracker::candidate_state(
        1280,
        800,
        DesktopVisualState::with_cursor(10, 10),
        None,
        &image,
    ));

    assert_eq!(
        tracker.damage_for_frame(
            1280,
            800,
            false,
            Some(OutputDamage::Empty),
            DesktopVisualState::with_cursor(20, 22),
            None,
        ),
        OutputDamage::Rects(vec![
            OutputRect::new(8, 9, 4, 3),
            OutputRect::new(18, 21, 4, 3),
        ])
    );
}

#[test]
fn output_damage_tracker_damages_old_and_new_bounds_when_cursor_image_changes() {
    let old_image = std::sync::Arc::new(
        oblivion_one::cursor_theme::CompositorCursorImage::from_argb8888(
            vec![0xff00_0000],
            1,
            1,
            0,
            0,
        )
        .unwrap(),
    );
    let new_image = std::sync::Arc::new(
        oblivion_one::cursor_theme::CompositorCursorImage::from_argb8888(
            vec![0xffff_0000; 3 * 2],
            3,
            2,
            0,
            0,
        )
        .unwrap(),
    );
    let mut tracker = EglOutputDamageTracker::with_cursor_image(old_image.clone());
    tracker.damage_for_frame(
        1280,
        800,
        true,
        None,
        DesktopVisualState::with_cursor(10, 10),
        None,
    );
    tracker.commit_presented(EglOutputDamageTracker::candidate_state(
        1280,
        800,
        DesktopVisualState::with_cursor(10, 10),
        None,
        &old_image,
    ));

    tracker.set_cursor_image(new_image);
    let damage = tracker.damage_for_frame(
        1280,
        800,
        false,
        Some(OutputDamage::Empty),
        DesktopVisualState::with_cursor(10, 10),
        None,
    );
    assert_eq!(damage.rect_count(), 1);
    assert_eq!(damage.pixels(1280, 800), Some(6));
}

#[test]
fn output_damage_tracker_does_not_damage_hidden_cursor_after_settling() {
    let image = oblivion_one::cursor_theme::CompositorCursorImage::builtin_fallback();
    let mut tracker = EglOutputDamageTracker::default();
    tracker.damage_for_frame(
        1280,
        800,
        true,
        None,
        DesktopVisualState::with_cursor(10, 10),
        None,
    );
    tracker.commit_presented(EglOutputDamageTracker::candidate_state(
        1280,
        800,
        DesktopVisualState::with_cursor(10, 10),
        None,
        &image,
    ));

    let hide_damage = tracker.damage_for_frame(
        1280,
        800,
        false,
        Some(OutputDamage::Empty),
        DesktopVisualState::wallpaper_only(),
        None,
    );
    assert_eq!(
        hide_damage.pixels(1280, 800),
        Some(u64::from(image.width) * u64::from(image.height))
    );
    tracker.commit_presented(EglOutputDamageTracker::candidate_state(
        1280,
        800,
        DesktopVisualState::wallpaper_only(),
        None,
        &image,
    ));
    assert_eq!(
        tracker.damage_for_frame(
            1280,
            800,
            false,
            Some(OutputDamage::Empty),
            DesktopVisualState::wallpaper_only(),
            None,
        ),
        OutputDamage::Empty
    );
}

#[test]
fn output_damage_tracker_repeats_candidate_damage_until_presented() {
    let mut tracker = EglOutputDamageTracker::default();
    tracker.damage_for_frame(
        1280,
        800,
        true,
        None,
        DesktopVisualState::with_cursor(10, 10),
        None,
    );
    tracker.commit_presented(EglOutputDamageTracker::candidate_state(
        1280,
        800,
        DesktopVisualState::with_cursor(10, 10),
        None,
        &oblivion_one::cursor_theme::CompositorCursorImage::builtin_fallback(),
    ));

    let first = tracker.damage_for_frame(
        1280,
        800,
        false,
        Some(OutputDamage::Empty),
        DesktopVisualState::with_cursor(20, 22),
        None,
    );
    let retry = tracker.damage_for_frame(
        1280,
        800,
        false,
        Some(OutputDamage::Empty),
        DesktopVisualState::with_cursor(20, 22),
        None,
    );

    assert_eq!(retry, first);
    assert_ne!(retry, OutputDamage::Empty);
}

#[test]
fn scene_cache_key_invalidates_when_surface_geometry_changes() {
    let initial_signature = EglSceneSurfaceSignature {
        surface_id: 7,
        commit_sequence: 1,
        buffer_id: 11,
        buffer_width: 800,
        buffer_height: 600,
        buffer_scale: 1,
        buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
        x: 10,
        y: 20,
        width: 800,
        height: 600,
        render_x: 0,
        render_y: 0,
        clip_x: 0,
        clip_y: 0,
        clip_width: 0,
        clip_height: 0,
        generation: 1,
    };
    let resized_signature = EglSceneSurfaceSignature {
        width: 420,
        height: 320,
        ..initial_signature
    };
    let key = EglSceneCacheKey::new(
        1280,
        800,
        9,
        120,
        &[initial_signature],
        OutputFramebufferOrigin::BottomLeft,
    );

    assert!(key.is_current(
        1280,
        800,
        9,
        120,
        &[initial_signature],
        OutputFramebufferOrigin::BottomLeft,
    ));
    assert!(!key.is_current(
        1280,
        800,
        9,
        120,
        &[resized_signature],
        OutputFramebufferOrigin::BottomLeft,
    ));
}

#[test]
fn scene_cache_key_invalidates_when_visual_assignment_changes() {
    let initial_signature = EglSceneSurfaceSignature {
        surface_id: 7,
        commit_sequence: 1,
        buffer_id: 11,
        buffer_width: 800,
        buffer_height: 600,
        buffer_scale: 1,
        buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
        x: 10,
        y: 20,
        width: 800,
        height: 600,
        render_x: 0,
        render_y: 0,
        clip_x: 0,
        clip_y: 0,
        clip_width: 0,
        clip_height: 0,
        generation: 1,
    };
    let cropped_signature = EglSceneSurfaceSignature {
        render_x: 5,
        render_y: 7,
        clip_x: 0,
        clip_y: 0,
        clip_width: 0,
        clip_height: 0,
        ..initial_signature
    };
    let key = EglSceneCacheKey::new(
        1280,
        800,
        9,
        120,
        &[initial_signature],
        OutputFramebufferOrigin::BottomLeft,
    );

    assert!(!key.is_current(
        1280,
        800,
        9,
        120,
        &[cropped_signature],
        OutputFramebufferOrigin::BottomLeft,
    ));
}

#[test]
fn scene_cache_key_invalidates_when_decoration_scene_changes() {
    let window_id = oblivion_one::compositor::WindowId::from_raw(21).expect("window id");
    let initial = DecorationSceneSnapshot::from_bounds(window_id, 7, 100, 80, 302, 227, 1);
    let changed = DecorationSceneSnapshot::from_bounds(window_id, 7, 100, 80, 302, 227, 2);
    let key = EglSceneCacheKey::new_with_decoration_snapshots(
        1280,
        800,
        9,
        120,
        &[],
        std::slice::from_ref(&initial),
        OutputFramebufferOrigin::BottomLeft,
    );

    assert!(key.is_current_with_decoration_snapshots(
        1280,
        800,
        9,
        120,
        &[],
        std::slice::from_ref(&initial),
        OutputFramebufferOrigin::BottomLeft,
    ));
    assert!(!key.is_current_with_decoration_snapshots(
        1280,
        800,
        9,
        120,
        &[],
        std::slice::from_ref(&changed),
        OutputFramebufferOrigin::BottomLeft,
    ));
}

#[test]
fn scene_cache_key_reuses_geometry_when_visible_buffer_identity_changes() {
    let initial = EglSceneSurfaceSignature {
        surface_id: 7,
        commit_sequence: 1,
        buffer_id: 11,
        buffer_width: 800,
        buffer_height: 600,
        buffer_scale: 1,
        buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
        x: 10,
        y: 20,
        width: 800,
        height: 600,
        render_x: 0,
        render_y: 0,
        clip_x: 0,
        clip_y: 0,
        clip_width: 0,
        clip_height: 0,
        generation: 1,
    };
    let replacement = EglSceneSurfaceSignature {
        buffer_id: 12,
        ..initial
    };
    let key = EglSceneCacheKey::new(
        1280,
        800,
        9,
        120,
        &[initial],
        OutputFramebufferOrigin::BottomLeft,
    );

    assert!(key.is_current(
        1280,
        800,
        9,
        120,
        &[replacement],
        OutputFramebufferOrigin::BottomLeft,
    ));
}

#[test]
fn scene_cache_key_reuses_geometry_when_content_generation_changes() {
    let signature = EglSceneSurfaceSignature {
        surface_id: 7,
        commit_sequence: 1,
        buffer_id: 11,
        buffer_width: 800,
        buffer_height: 600,
        buffer_scale: 1,
        buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
        x: 10,
        y: 20,
        width: 800,
        height: 600,
        render_x: 0,
        render_y: 0,
        clip_x: 0,
        clip_y: 0,
        clip_width: 0,
        clip_height: 0,
        generation: 1,
    };
    let key = EglSceneCacheKey::new(
        1280,
        800,
        9,
        120,
        &[signature],
        OutputFramebufferOrigin::BottomLeft,
    );

    assert!(key.is_current(
        1280,
        800,
        10,
        120,
        &[EglSceneSurfaceSignature {
            commit_sequence: 2,
            buffer_id: 12,
            generation: 2,
            ..signature
        }],
        OutputFramebufferOrigin::BottomLeft,
    ));
}

#[test]
fn scene_cache_key_invalidates_when_framebuffer_origin_changes() {
    let signature = EglSceneSurfaceSignature {
        surface_id: 7,
        commit_sequence: 1,
        buffer_id: 11,
        buffer_width: 800,
        buffer_height: 600,
        buffer_scale: 1,
        buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
        x: 10,
        y: 20,
        width: 800,
        height: 600,
        render_x: 0,
        render_y: 0,
        clip_x: 0,
        clip_y: 0,
        clip_width: 0,
        clip_height: 0,
        generation: 1,
    };
    let key = EglSceneCacheKey::new(
        1280,
        800,
        9,
        120,
        &[signature],
        OutputFramebufferOrigin::BottomLeft,
    );

    assert!(!key.is_current(
        1280,
        800,
        9,
        120,
        &[signature],
        OutputFramebufferOrigin::TopLeftScanout,
    ));
}

#[test]
fn scene_cache_key_invalidates_when_presentation_geometry_changes() {
    let key = EglSceneCacheKey::new_with_presentation(
        1280,
        800,
        9,
        120,
        &[],
        11,
        OutputFramebufferOrigin::BottomLeft,
    );

    assert!(key.is_current_with_decorations(
        1280,
        800,
        9,
        120,
        &[],
        &[],
        &[],
        11,
        OutputFramebufferOrigin::BottomLeft,
    ));
    assert!(!key.is_current_with_decorations(
        1280,
        800,
        9,
        120,
        &[],
        &[],
        &[],
        12,
        OutputFramebufferOrigin::BottomLeft,
    ));
}

#[test]
fn scene_cache_key_invalidates_when_dock_overlay_partition_changes() {
    let surface = EglSceneSurfaceSignature {
        surface_id: 703,
        commit_sequence: 1,
        buffer_id: 11,
        buffer_width: 64,
        buffer_height: 64,
        buffer_scale: 1,
        buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
        x: 608,
        y: 368,
        width: 64,
        height: 64,
        render_x: 608,
        render_y: 368,
        clip_x: 0,
        clip_y: 0,
        clip_width: 0,
        clip_height: 0,
        generation: 1,
    };
    let surfaces = std::slice::from_ref(&surface);
    let base = EglSceneCacheKey::new_with_decorations_and_external_overlay_ids(
        1280,
        800,
        9,
        120,
        surfaces,
        11,
        &[],
        &[],
        &[],
        OutputFramebufferOrigin::BottomLeft,
    );
    let promoted = EglSceneCacheKey::new_with_decorations_and_external_overlay_ids(
        1280,
        800,
        9,
        120,
        surfaces,
        11,
        &[703],
        &[],
        &[],
        OutputFramebufferOrigin::BottomLeft,
    );
    let restored = EglSceneCacheKey::new_with_decorations_and_external_overlay_ids(
        1280,
        800,
        9,
        120,
        surfaces,
        11,
        &[],
        &[],
        &[],
        OutputFramebufferOrigin::BottomLeft,
    );

    assert!(!base.is_current_with_decorations_and_external_overlay_ids(
        1280,
        800,
        9,
        120,
        surfaces,
        &[703],
        &[],
        &[],
        11,
        OutputFramebufferOrigin::BottomLeft,
    ));
    assert!(
        promoted.is_current_with_decorations_and_external_overlay_ids(
            1280,
            800,
            9,
            120,
            surfaces,
            &[703],
            &[],
            &[],
            11,
            OutputFramebufferOrigin::BottomLeft,
        )
    );
    assert!(
        !promoted.is_current_with_decorations_and_external_overlay_ids(
            1280,
            800,
            9,
            120,
            surfaces,
            &[],
            &[],
            &[],
            11,
            OutputFramebufferOrigin::BottomLeft,
        )
    );
    assert!(
        restored.is_current_with_decorations_and_external_overlay_ids(
            1280,
            800,
            9,
            120,
            surfaces,
            &[],
            &[],
            &[],
            11,
            OutputFramebufferOrigin::BottomLeft,
        )
    );
}
