use super::*;
#[rustfmt::skip]
use crate::egl_renderer::{BufferAge, EglPartialRepaintCapabilities, PartialRepaintPlanner, RepaintMode};
#[test]
fn direct_plane_validation_key_changes_for_modifier_and_generation() {
    let first = DirectPlaneValidationKey {
        output_generation: 1,
        crtc_id: 7,
        primary_plane_id: 11,
        mode_width: 1920,
        mode_height: 1080,
        format: 0x3432_5241,
        modifier: 0,
        buffer_width: 1920,
        buffer_height: 1080,
        plane_layout_hash: 3,
        cursor_atomic_key: None,
        synchronization_key: 4,
        presentation_mode: OutputPresentationMode::Vsync,
        content_type: DrmContentType::Graphics,
    };
    let modifier_changed = DirectPlaneValidationKey {
        modifier: 7,
        ..first
    };
    assert_ne!(first, modifier_changed);
    let generation_changed = DirectPlaneValidationKey {
        output_generation: 2,
        ..first
    };
    assert_ne!(first, generation_changed);
}
#[test]
fn connected_connector_for_card_prefers_connected_matching_card_output() {
    let root = std::env::current_dir()
        .unwrap()
        .join("target")
        .join("native-output-tests")
        .join(std::process::id().to_string());
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("card1-DP-1")).unwrap();
    fs::create_dir_all(root.join("card1-HDMI-A-1")).unwrap();
    fs::create_dir_all(root.join("card0-DP-1")).unwrap();
    fs::write(root.join("card1-DP-1/status"), "connected\n").unwrap();
    fs::write(root.join("card1-DP-1/enabled"), "enabled\n").unwrap();
    fs::write(root.join("card1-DP-1/modes"), "1920x1080\n1280x720\n").unwrap();
    fs::write(root.join("card1-DP-1/vrr_capable"), "1\n").unwrap();
    fs::write(root.join("card1-HDMI-A-1/status"), "disconnected\n").unwrap();
    fs::write(root.join("card0-DP-1/status"), "connected\n").unwrap();
    fs::write(root.join("card0-DP-1/modes"), "800x600\n").unwrap();

    let connector = connected_connector_for_card(Some(Path::new("/dev/dri/card1")), &root)
        .expect("connected card1 output should be detected");
    let _ = fs::remove_dir_all(&root);

    assert_eq!(connector.name, "card1-DP-1");
    assert_eq!(connector.enabled.as_deref(), Some("enabled"));
    assert_eq!(connector.preferred_mode(), Some("1920x1080"));
    assert_eq!(connector.vrr_capable, Some(true));
}

#[test]
fn matching_render_node_for_card_uses_same_drm_device_directory() {
    let root = std::env::current_dir()
        .unwrap()
        .join("target")
        .join("native-render-node-tests")
        .join(std::process::id().to_string());
    let sysfs = root.join("sys");
    let dri = root.join("dev").join("dri");
    let drm_dir = sysfs.join("card2").join("device").join("drm");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&drm_dir).unwrap();
    fs::create_dir_all(&dri).unwrap();
    fs::create_dir_all(drm_dir.join("renderD130")).unwrap();
    fs::create_dir_all(drm_dir.join("card2")).unwrap();

    let render = matching_render_node_for_card(Path::new("/dev/dri/card2"), &sysfs, &dri);
    let _ = fs::remove_dir_all(&root);

    assert_eq!(render, Some(dri.join("renderD130")));
}

#[test]
fn native_vrr_preference_parses_policy_values() {
    assert_eq!(
        NativeVrrPreference::parse("auto"),
        NativeVrrPreference::Auto
    );
    assert_eq!(NativeVrrPreference::parse("1"), NativeVrrPreference::On);
    assert_eq!(NativeVrrPreference::parse("true"), NativeVrrPreference::On);
    assert_eq!(NativeVrrPreference::parse("0"), NativeVrrPreference::Off);
    assert_eq!(
        NativeVrrPreference::parse("false"),
        NativeVrrPreference::Off
    );
    assert_eq!(
        NativeVrrPreference::parse("unknown"),
        NativeVrrPreference::Auto
    );
}

#[test]
fn native_vrr_plan_auto_enables_only_when_connector_is_capable() {
    assert_eq!(
        NativeVrrPlan::choose(NativeVrrPreference::Auto, Some(true)),
        NativeVrrPlan {
            requested: NativeVrrPreference::Auto,
            supported: true,
            planned_enabled: true,
        }
    );
    assert_eq!(
        NativeVrrPlan::choose(NativeVrrPreference::Auto, Some(false)),
        NativeVrrPlan {
            requested: NativeVrrPreference::Auto,
            supported: false,
            planned_enabled: false,
        }
    );
}

#[test]
fn drm_mode_name_reads_nul_terminated_kernel_mode_name() {
    let mut mode = drm_sys::drm_mode_modeinfo::default();
    for (index, byte) in b"2560x1440\0ignored".iter().enumerate() {
        mode.name[index] = *byte as _;
    }

    assert_eq!(drm_mode_name(&mode), "2560x1440");
}

#[test]
fn native_perf_log_env_accepts_truthy_values() {
    assert!(native_perf_log_value_enabled("1"));
    assert!(native_perf_log_value_enabled("true"));
    assert!(native_perf_log_value_enabled("debug"));
    assert!(!native_perf_log_value_enabled("0"));
    assert!(!native_perf_log_value_enabled("false"));
    assert!(!native_perf_log_value_enabled(""));
}

#[test]
fn native_perf_line_formats_structured_fields() {
    let line = native_perf_line(
        "app.spawn",
        &[
            NativePerfField::str("program", "zen browser"),
            NativePerfField::u64("pid", 4242),
            NativePerfField::str("app_policy", "accelerated"),
        ],
    );

    assert_eq!(
        line,
        "perf app.spawn program=\"zen browser\" pid=4242 app_policy=accelerated"
    );
}

#[test]
fn native_app_gpu_preference_parses_explicit_values() {
    assert_eq!(
        CompositorAppGpuPreference::from_native_env_value(None),
        CompositorAppGpuPreference::Auto
    );
    assert_eq!(
        CompositorAppGpuPreference::parse("accelerated"),
        CompositorAppGpuPreference::Accelerated
    );
    assert_eq!(
        CompositorAppGpuPreference::parse("gpu"),
        CompositorAppGpuPreference::Accelerated
    );
    assert_eq!(
        CompositorAppGpuPreference::parse("auto"),
        CompositorAppGpuPreference::Auto
    );
    assert_eq!(
        CompositorAppGpuPreference::parse("cpu"),
        CompositorAppGpuPreference::CpuOnly
    );
    assert_eq!(
        CompositorAppGpuPreference::parse("software"),
        CompositorAppGpuPreference::CpuOnly
    );
    assert_eq!(
        CompositorAppGpuPreference::parse("unknown"),
        CompositorAppGpuPreference::Auto
    );
}

#[test]
fn native_app_gpu_policy_resolves_from_active_scanout_backend() {
    assert_eq!(
        resolve_native_app_gpu_policy(
            CompositorAppGpuPreference::Auto,
            NativeScanoutKind::AtomicEglGbmExplicit,
        )
        .unwrap(),
        EffectiveCompositorAppGpuPolicy::Accelerated
    );
    assert_eq!(
        resolve_native_app_gpu_policy(
            CompositorAppGpuPreference::Auto,
            NativeScanoutKind::GbmCpuWritePageFlip,
        )
        .unwrap(),
        EffectiveCompositorAppGpuPolicy::CpuOnly
    );
    assert_eq!(
        resolve_native_app_gpu_policy(
            CompositorAppGpuPreference::CpuOnly,
            NativeScanoutKind::AtomicEglGbmExplicit,
        )
        .unwrap(),
        EffectiveCompositorAppGpuPolicy::CpuOnly
    );
    assert!(
        resolve_native_app_gpu_policy(
            CompositorAppGpuPreference::Accelerated,
            NativeScanoutKind::DumbFramebuffer,
        )
        .is_err()
    );
}

#[test]
fn native_launch_request_ignores_empty_command() {
    assert_eq!(
        native_launch_request(
            Vec::new(),
            EffectiveCompositorAppGpuPolicy::CpuOnly,
            NativeLaunchSource::Startup,
        ),
        None
    );
}

#[test]
fn native_launch_request_preserves_args_policy_and_source() {
    let request = native_launch_request(
        vec![
            "kitty".to_string(),
            "--title".to_string(),
            "two words".to_string(),
        ],
        EffectiveCompositorAppGpuPolicy::Accelerated,
        NativeLaunchSource::Startup,
    )
    .unwrap();

    assert_eq!(request.program, "kitty");
    assert_eq!(request.command, "kitty --title 'two words'");
    assert_eq!(request.argv[2], "two words");
    assert_eq!(
        request.gpu_policy,
        EffectiveCompositorAppGpuPolicy::Accelerated
    );
    assert_eq!(request.source, NativeLaunchSource::Startup);
}

#[test]
fn native_runtime_error_includes_stage_backend_frame_and_recovery_command() {
    let error = native_runtime_error(
        NativeRuntimeStage::Present,
        NativeScanoutKind::AtomicEglGbmExplicit,
        42,
        1842,
        io::Error::other("page flip failed"),
    );
    let message = error.to_string();

    assert!(message.contains("fatal native GPU runtime error"));
    assert!(message.contains("stage=present"));
    assert!(message.contains("backend=atomic-egl-gbm-explicit"));
    assert!(message.contains("crtc=42"));
    assert!(message.contains("frame=1842"));
    assert!(message.contains("OBLIVION_ONE_SCANOUT_BACKEND=cpu"));
}

#[test]
fn native_damage_accumulator_reports_full_surface_damage() {
    let surface = test_renderable_surface(1, 20, 10, 80, 40, RenderableSurfaceDamage::Full);
    let mut damage = NativeDamageAccumulator::for_output(200, 120);

    damage.add_surface(&surface, (20, 10));

    assert_eq!(
        damage.summary(),
        NativeDamageSummary {
            kind: NativeDamageKind::SurfaceDamage,
            rects: 1,
            pixels: 3_200,
        }
    );
}

#[test]
fn native_damage_accumulator_maps_partial_surface_damage_to_output() {
    let surface = test_renderable_surface(
        2,
        0,
        0,
        100,
        50,
        RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
            x: 10,
            y: 5,
            width: 30,
            height: 20,
        }]),
    );
    let mut damage = NativeDamageAccumulator::for_output(200, 120);

    damage.add_surface(&surface, (72, 72));

    assert_eq!(
        damage.rects(),
        &[NativeDamageRect {
            x: 82,
            y: 77,
            width: 30,
            height: 20,
        }]
    );
    assert_eq!(damage.summary().pixels, 600);
}

#[test]
fn native_damage_accumulator_maps_render_scene_element_damage_to_output() {
    let surface = test_renderable_surface(
        2,
        0,
        0,
        100,
        50,
        RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
            x: 10,
            y: 5,
            width: 30,
            height: 20,
        }]),
    );
    let elements = render_scene_elements_for_surfaces(std::slice::from_ref(&surface), 1.0);

    let damage = NativeDamageAccumulator::from_render_elements(200, 120, &elements);

    assert_eq!(
        damage.rects(),
        &[NativeDamageRect {
            x: 82,
            y: 77,
            width: 30,
            height: 20,
        }]
    );
}

#[test]
fn native_damage_accumulator_clips_partial_surface_damage_to_output() {
    let surface = test_renderable_surface(
        3,
        0,
        0,
        80,
        40,
        RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
            x: 60,
            y: 20,
            width: 20,
            height: 20,
        }]),
    );
    let mut damage = NativeDamageAccumulator::for_output(100, 80);

    damage.add_surface(&surface, (90, 70));

    assert!(damage.rects().is_empty());
    assert_eq!(damage.summary().kind, NativeDamageKind::Empty);
}

#[test]
fn native_damage_summary_full_output_fallback_counts_output_pixels() {
    assert_eq!(
        NativeOutputDamage::full_output(1920, 1080).summary(),
        NativeDamageSummary {
            kind: NativeDamageKind::FullOutput,
            rects: 1,
            pixels: 2_073_600,
        }
    );
}

#[test]
fn native_output_damage_for_cursor_motion_is_not_full_output() {
    let damage = native_output_damage_for_repaint(
        1920,
        1080,
        &[],
        &[],
        RenderGenerationCause::CursorMotion,
        true,
    );

    assert_ne!(damage.kind, NativeDamageKind::FullOutput);
}

#[test]
fn native_output_damage_for_client_cursor_motion_covers_old_and_new_bounds() {
    let previous = NativeClientCursorDamageState {
        surface_id: 9,
        generation: 1,
        hotspot_x: 0,
        hotspot_y: 0,
        rect: Some(NativeDamageRect {
            x: 100,
            y: 100,
            width: 32,
            height: 32,
        }),
    };
    let current = NativeClientCursorDamageState {
        rect: Some(NativeDamageRect {
            x: 200,
            y: 100,
            width: 32,
            height: 32,
        }),
        ..previous
    };

    let damage = native_output_damage_for_repaint_with_cursor(
        1920,
        1080,
        &[],
        &[],
        RenderGenerationCause::CursorMotion,
        false,
        NativeCursorDamageBounds {
            previous_client: Some(previous),
            client: Some(current),
            ..Default::default()
        },
    );

    assert_eq!(damage.kind, NativeDamageKind::SurfaceDamage);
    assert_eq!(damage.rects.len(), 2);
    assert!(damage.rects.contains(&previous.rect.unwrap()));
    assert!(damage.rects.contains(&current.rect.unwrap()));
}

#[test]
fn native_output_damage_for_software_theme_cursor_motion_is_bounded() {
    let old = NativeDamageRect {
        x: 10,
        y: 20,
        width: 24,
        height: 24,
    };
    let new = NativeDamageRect {
        x: 40,
        y: 20,
        width: 24,
        height: 24,
    };
    let damage = native_output_damage_for_repaint_with_cursor(
        1920,
        1080,
        &[],
        &[],
        RenderGenerationCause::CursorMotion,
        false,
        NativeCursorDamageBounds {
            previous_software: Some(old),
            software: Some(new),
            ..Default::default()
        },
    );

    assert_eq!(damage.kind, NativeDamageKind::SurfaceDamage);
    assert_eq!(damage.rects.len(), 2);
    assert!(damage.rects.contains(&old));
    assert!(damage.rects.contains(&new));
    assert!(damage.pixels < 1920 * 1080);
}

#[test]
fn native_output_damage_for_window_move_covers_old_and_new_surface_bounds() {
    let previous = test_renderable_surface(7, 0, 0, 120, 80, RenderableSurfaceDamage::Full);
    let current = test_renderable_surface(7, 200, 100, 120, 80, RenderableSurfaceDamage::Full);
    let previous_origin = surface_origins(std::slice::from_ref(&previous))[0];
    let current_origin = surface_origins(std::slice::from_ref(&current))[0];

    let damage = native_output_damage_for_repaint(
        400,
        300,
        std::slice::from_ref(&previous),
        std::slice::from_ref(&current),
        RenderGenerationCause::WindowMove,
        true,
    );

    assert_eq!(damage.kind, NativeDamageKind::SurfaceDamage);
    assert_eq!(
        damage.rects,
        vec![
            NativeDamageRect {
                x: previous_origin.0,
                y: previous_origin.1,
                width: 120,
                height: 80,
            },
            NativeDamageRect {
                x: current_origin.0,
                y: current_origin.1,
                width: 120,
                height: 80,
            },
        ]
    );
}

#[test]
fn native_decoration_damage_covers_old_new_state_change_and_disappearance() {
    let window_id = WindowId::from_raw(41).expect("non-zero test window id");
    let previous = DecorationSceneSnapshot::from_bounds(window_id, 7, 100, 80, 302, 227, 10);
    let moved = DecorationSceneSnapshot::from_bounds(window_id, 7, 220, 140, 302, 227, 10);
    let damage = NativeDamageAccumulator::from_decoration_bounds_changes(
        640,
        480,
        std::slice::from_ref(&previous),
        std::slice::from_ref(&moved),
    )
    .into_output_damage();

    assert!(
        damage
            .rects
            .iter()
            .any(|rect| native_damage_rect_contains(*rect, 100, 80))
    );
    assert!(
        damage
            .rects
            .iter()
            .any(|rect| native_damage_rect_contains(*rect, 220, 140))
    );

    let state_changed = DecorationSceneSnapshot::from_bounds(window_id, 7, 220, 140, 302, 227, 11);
    let state_damage = NativeDamageAccumulator::from_decoration_bounds_changes(
        640,
        480,
        std::slice::from_ref(&moved),
        std::slice::from_ref(&state_changed),
    )
    .into_output_damage();
    assert!(!state_damage.is_empty());
    assert!(state_damage.pixels < 640 * 480);

    let disappearance = NativeDamageAccumulator::from_decoration_bounds_changes(
        640,
        480,
        std::slice::from_ref(&state_changed),
        &[],
    )
    .into_output_damage();
    assert!(disappearance.rects.contains(&NativeDamageRect {
        x: 220,
        y: 140,
        width: 302,
        height: 227,
    }));
}

#[test]
fn native_output_damage_reproduces_ssd_trailing_titlebar_on_reused_buffer() {
    const WIDTH: u32 = 640;
    const HEIGHT: u32 = 480;
    const TITLEBAR_HEIGHT: i32 = 26;
    const FRAME_BORDER: i32 = 1;
    const BACKGROUND: u32 = 0xff12_151c;
    const TITLEBAR: u32 = 0xff33_3333;
    const CLIENT: u32 = 0xff65_7890;

    let mut previous_position = (72, 72);
    let mut previous_surface = test_renderable_surface(
        77,
        previous_position.0,
        previous_position.1,
        300,
        200,
        RenderableSurfaceDamage::Full,
    );
    let window_id = WindowId::from_raw(77).expect("non-zero test window id");
    let mut previous_decoration = DecorationSceneSnapshot::from_bounds(
        window_id,
        77,
        previous_position.0 - FRAME_BORDER,
        previous_position.1 - TITLEBAR_HEIGHT,
        302,
        227,
        1,
    );
    let mut retained = vec![BACKGROUND; (WIDTH * HEIGHT) as usize];
    paint_ssd_scene(
        &mut retained,
        WIDTH,
        HEIGHT,
        previous_position,
        TITLEBAR_HEIGHT,
        FRAME_BORDER,
        TITLEBAR,
        CLIENT,
        None,
    );

    for step in 1..=30 {
        let current_position = (72 + step * 7, 72 + step * 3);
        let current_surface = test_renderable_surface(
            77,
            current_position.0,
            current_position.1,
            300,
            200,
            RenderableSurfaceDamage::Full,
        );
        let current_decoration = DecorationSceneSnapshot::from_bounds(
            window_id,
            77,
            current_position.0 - FRAME_BORDER,
            current_position.1 - TITLEBAR_HEIGHT,
            302,
            227,
            1,
        );
        let damage = native_output_damage_for_scene_and_cursor_with_decorations(
            WIDTH,
            HEIGHT,
            std::slice::from_ref(&previous_surface),
            std::slice::from_ref(&current_surface),
            std::slice::from_ref(&previous_decoration),
            std::slice::from_ref(&current_decoration),
            true,
            NativeCursorDamageBounds::default(),
        );
        assert!(
            damage.rects.iter().any(|rect| native_damage_rect_contains(
                *rect,
                previous_position.0 + 10,
                previous_position.1 - TITLEBAR_HEIGHT / 2,
            )),
            "step {step} must repair the old titlebar-only pixel"
        );

        paint_ssd_scene(
            &mut retained,
            WIDTH,
            HEIGHT,
            current_position,
            TITLEBAR_HEIGHT,
            FRAME_BORDER,
            TITLEBAR,
            CLIENT,
            Some(&damage.rects),
        );

        let mut reference = vec![BACKGROUND; (WIDTH * HEIGHT) as usize];
        paint_ssd_scene(
            &mut reference,
            WIDTH,
            HEIGHT,
            current_position,
            TITLEBAR_HEIGHT,
            FRAME_BORDER,
            TITLEBAR,
            CLIENT,
            None,
        );

        assert_eq!(
            retained, reference,
            "partial SSD repaint diverged from the full reference at movement step {step}; old titlebar pixels were not repaired"
        );

        previous_surface = current_surface;
        previous_decoration = current_decoration;
        previous_position = current_position;
    }
}

#[test]
fn render_ahead_oversized_ssd_repair_matches_full_reference() {
    const WIDTH: u32 = 1920;
    const HEIGHT: u32 = 1080;
    const TITLEBAR_HEIGHT: i32 = 26;
    const FRAME_BORDER: i32 = 1;
    const ROOT_X: i32 = -200;
    const ROOT_Y: i32 = 120;
    const PRESENTED_A_WIDTH: u32 = 2100;
    const RENDERED_B_WIDTH: u32 = 1950;
    const CURRENT_C_WIDTH: u32 = 1750;

    let presented_a = test_renderable_surface(
        88,
        ROOT_X - 72,
        ROOT_Y - 72,
        PRESENTED_A_WIDTH,
        420,
        RenderableSurfaceDamage::Full,
    );
    let rendered_b = test_renderable_surface(
        88,
        ROOT_X - 72,
        ROOT_Y - 72,
        RENDERED_B_WIDTH,
        420,
        RenderableSurfaceDamage::Full,
    );
    let current_c = test_renderable_surface(
        88,
        ROOT_X - 72,
        ROOT_Y - 72,
        CURRENT_C_WIDTH,
        420,
        RenderableSurfaceDamage::Full,
    );
    assert!(presented_a.width > rendered_b.width);
    assert!(rendered_b.width > current_c.width);
    let window_id = WindowId::from_raw(88).expect("non-zero test window id");
    let decoration = |width: u32| {
        DecorationSceneSnapshot::from_bounds(
            window_id,
            88,
            ROOT_X - FRAME_BORDER,
            ROOT_Y - TITLEBAR_HEIGHT,
            width.saturating_add((FRAME_BORDER * 2) as u32),
            420 + TITLEBAR_HEIGHT as u32 + (FRAME_BORDER * 2) as u32,
            1,
        )
    };
    assert_eq!(
        surface_origins(std::slice::from_ref(&rendered_b)),
        vec![(ROOT_X, ROOT_Y)]
    );

    let presented_a_scene = NativeSceneSnapshot::from_surfaces(
        std::slice::from_ref(&presented_a),
        vec![decoration(PRESENTED_A_WIDTH)],
    );
    let rendered_b_scene = NativeSceneSnapshot::from_surfaces(
        std::slice::from_ref(&rendered_b),
        vec![decoration(RENDERED_B_WIDTH)],
    );
    let mut history = NativeSceneHistory::new(NativeFrameSceneSnapshot {
        frame_id: 1,
        render_generation: 1,
        scene: presented_a_scene,
        cursor_damage: NativeCursorDamageBounds::default(),
    });
    history.replace_ready(NativeFrameSceneSnapshot {
        frame_id: 2,
        render_generation: 2,
        scene: rendered_b_scene.clone(),
        cursor_damage: NativeCursorDamageBounds::default(),
    });
    assert!(history.queue_submission(2));
    assert_eq!(history.presented_scene().surfaces[0].surface_id, 88);

    let mut partial = vec![0xff12_151c; (WIDTH * HEIGHT) as usize];
    paint_oversized_ssd_scene(
        &mut partial,
        WIDTH,
        HEIGHT,
        ROOT_X,
        ROOT_Y,
        PRESENTED_A_WIDTH,
        TITLEBAR_HEIGHT,
        FRAME_BORDER,
        None,
    );

    let current_scene = NativeSceneSnapshot::from_surfaces(
        std::slice::from_ref(&current_c),
        vec![decoration(CURRENT_C_WIDTH)],
    );
    let b_to_c_damage = native_output_damage_for_scene_snapshots(
        WIDTH,
        HEIGHT,
        &rendered_b_scene,
        &current_scene,
        NativeCursorDamageBounds::default(),
    );
    assert!(
        !b_to_c_damage
            .rects
            .iter()
            .any(|rect| native_damage_rect_contains(
                *rect,
                ROOT_X + PRESENTED_A_WIDTH as i32 - 70,
                ROOT_Y - TITLEBAR_HEIGHT / 2,
            )),
        "B-to-C damage unexpectedly covers the A-only button sample: {:?}",
        b_to_c_damage.rects
    );
    let damage = native_output_damage_for_scene_snapshots(
        WIDTH,
        HEIGHT,
        history.presented_scene(),
        &current_scene,
        NativeCursorDamageBounds::default(),
    );
    assert!(damage.rects.iter().any(|rect| {
        native_damage_rect_contains(
            *rect,
            ROOT_X + PRESENTED_A_WIDTH as i32 - 70,
            ROOT_Y - TITLEBAR_HEIGHT / 2,
        )
    }));
    paint_oversized_ssd_scene(
        &mut partial,
        WIDTH,
        HEIGHT,
        ROOT_X,
        ROOT_Y,
        CURRENT_C_WIDTH,
        TITLEBAR_HEIGHT,
        FRAME_BORDER,
        Some(&damage.rects),
    );

    let mut reference = vec![0xff12_151c; (WIDTH * HEIGHT) as usize];
    paint_oversized_ssd_scene(
        &mut reference,
        WIDTH,
        HEIGHT,
        ROOT_X,
        ROOT_Y,
        CURRENT_C_WIDTH,
        TITLEBAR_HEIGHT,
        FRAME_BORDER,
        None,
    );

    let stale_button_x = ROOT_X + PRESENTED_A_WIDTH as i32 - 70;
    let stale_edge_x = ROOT_X + PRESENTED_A_WIDTH as i32 - FRAME_BORDER - 1;
    let sample_y = ROOT_Y - TITLEBAR_HEIGHT / 2;
    assert_eq!(
        partial[(sample_y * WIDTH as i32 + stale_button_x) as usize],
        reference[(sample_y * WIDTH as i32 + stale_button_x) as usize],
        "the A-only button region must be repaired from presented-scene history"
    );
    assert_eq!(
        partial[(sample_y * WIDTH as i32 + stale_edge_x) as usize],
        reference[(sample_y * WIDTH as i32 + stale_edge_x) as usize],
        "the A-only titlebar edge must be repaired from presented-scene history"
    );
    let mismatches = partial
        .iter()
        .zip(&reference)
        .filter(|(partial, reference)| partial != reference)
        .count();
    assert_eq!(
        mismatches, 0,
        "partial A-backed repaint must equal the clean C reference"
    );
}

#[test]
fn rejected_oversized_ssd_retry_matches_full_reference() {
    const WIDTH: u32 = 1920;
    const HEIGHT: u32 = 1080;
    const ROOT_X: i32 = -200;
    const ROOT_Y: i32 = 160;
    const TITLEBAR_HEIGHT: i32 = 26;
    const FRAME_BORDER: i32 = 1;
    let window_id = WindowId::from_raw(92).expect("non-zero test window id");
    let decoration = |width: u32| {
        DecorationSceneSnapshot::from_bounds(
            window_id,
            92,
            ROOT_X - FRAME_BORDER,
            ROOT_Y - TITLEBAR_HEIGHT,
            width.saturating_add((FRAME_BORDER * 2) as u32),
            420 + TITLEBAR_HEIGHT as u32 + (FRAME_BORDER * 2) as u32,
            1,
        )
    };
    let surface = |width: u32, generation: u64| {
        let mut surface = test_renderable_surface(
            92,
            ROOT_X - 72,
            ROOT_Y - 72,
            width,
            420,
            RenderableSurfaceDamage::Empty,
        );
        surface.generation = generation;
        surface
    };
    let presented_surface = surface(2200, 30);
    let retry_surface = surface(1400, 31);
    let presented_scene = NativeSceneSnapshot::from_surfaces(
        std::slice::from_ref(&presented_surface),
        vec![decoration(2200)],
    );
    let retry_scene = NativeSceneSnapshot::from_surfaces(
        std::slice::from_ref(&retry_surface),
        vec![decoration(1400)],
    );
    let mut history = NativeSceneHistory::new(NativeFrameSceneSnapshot {
        frame_id: 1,
        render_generation: 1,
        scene: presented_scene,
        cursor_damage: NativeCursorDamageBounds::default(),
    });
    history.replace_ready(NativeFrameSceneSnapshot {
        frame_id: 2,
        render_generation: 44,
        scene: retry_scene.clone(),
        cursor_damage: NativeCursorDamageBounds::default(),
    });
    assert!(history.queue_submission(220));
    assert!(history.discard_submission(220));
    history.replace_ready(NativeFrameSceneSnapshot {
        frame_id: 3,
        render_generation: 44,
        scene: retry_scene.clone(),
        cursor_damage: NativeCursorDamageBounds::default(),
    });

    let damage = native_output_damage_for_scene_snapshots(
        WIDTH,
        HEIGHT,
        history.presented_scene(),
        &retry_scene,
        NativeCursorDamageBounds::default(),
    );
    let mut partial = vec![0xff12_151c; (WIDTH * HEIGHT) as usize];
    paint_oversized_ssd_scene(
        &mut partial,
        WIDTH,
        HEIGHT,
        ROOT_X,
        ROOT_Y,
        2200,
        TITLEBAR_HEIGHT,
        FRAME_BORDER,
        None,
    );
    paint_oversized_ssd_scene(
        &mut partial,
        WIDTH,
        HEIGHT,
        ROOT_X,
        ROOT_Y,
        1400,
        TITLEBAR_HEIGHT,
        FRAME_BORDER,
        Some(&damage.rects),
    );
    let mut reference = vec![0xff12_151c; (WIDTH * HEIGHT) as usize];
    paint_oversized_ssd_scene(
        &mut reference,
        WIDTH,
        HEIGHT,
        ROOT_X,
        ROOT_Y,
        1400,
        TITLEBAR_HEIGHT,
        FRAME_BORDER,
        None,
    );
    let stale_button_sample =
        ((ROOT_Y - TITLEBAR_HEIGHT + 13) * WIDTH as i32 + ROOT_X + 2200 - 150) as usize;
    assert_eq!(
        partial[stale_button_sample], reference[stale_button_sample],
        "the old traffic-light cluster must be repaired after the SSD shrink"
    );
    let stale_titlebar_edge_sample =
        ((ROOT_Y - TITLEBAR_HEIGHT + 13) * WIDTH as i32 + WIDTH as i32 - 1) as usize;
    assert_eq!(
        partial[stale_titlebar_edge_sample], reference[stale_titlebar_edge_sample],
        "the old visible titlebar right edge must be repaired after the SSD shrink"
    );
    assert_eq!(partial, reference);
}

#[test]
fn rejected_oversized_csd_retry_matches_full_reference() {
    const WIDTH: u32 = 1920;
    const HEIGHT: u32 = 1080;
    const ROOT_X: i32 = -200;
    const ROOT_Y: i32 = 160;
    let surface = |width: u32, generation: u64| {
        let mut surface = test_renderable_surface(
            93,
            ROOT_X - 72,
            ROOT_Y - 72,
            width,
            420,
            RenderableSurfaceDamage::Empty,
        );
        surface.generation = generation;
        surface
    };
    let presented_surface = surface(2200, 50);
    let retry_surface = surface(1400, 51);
    let presented_scene =
        NativeSceneSnapshot::from_surfaces(std::slice::from_ref(&presented_surface), Vec::new());
    let retry_scene =
        NativeSceneSnapshot::from_surfaces(std::slice::from_ref(&retry_surface), Vec::new());
    let mut history = NativeSceneHistory::new(NativeFrameSceneSnapshot {
        frame_id: 1,
        render_generation: 1,
        scene: presented_scene,
        cursor_damage: NativeCursorDamageBounds::default(),
    });
    history.replace_ready(NativeFrameSceneSnapshot {
        frame_id: 2,
        render_generation: 45,
        scene: retry_scene.clone(),
        cursor_damage: NativeCursorDamageBounds::default(),
    });
    assert!(history.queue_submission(230));
    assert!(history.discard_submission(230));
    history.replace_ready(NativeFrameSceneSnapshot {
        frame_id: 3,
        render_generation: 45,
        scene: retry_scene.clone(),
        cursor_damage: NativeCursorDamageBounds::default(),
    });

    let damage = native_output_damage_for_scene_snapshots(
        WIDTH,
        HEIGHT,
        history.presented_scene(),
        &retry_scene,
        NativeCursorDamageBounds::default(),
    );
    let mut partial = vec![0xff12_151c; (WIDTH * HEIGHT) as usize];
    paint_client_scene(
        &mut partial,
        WIDTH,
        HEIGHT,
        ROOT_X,
        ROOT_Y,
        2200,
        420,
        0xff4c_7890,
        None,
    );
    paint_client_scene(
        &mut partial,
        WIDTH,
        HEIGHT,
        ROOT_X,
        ROOT_Y,
        1400,
        420,
        0xff78_90a4,
        Some(&damage.rects),
    );
    let mut reference = vec![0xff12_151c; (WIDTH * HEIGHT) as usize];
    paint_client_scene(
        &mut reference,
        WIDTH,
        HEIGHT,
        ROOT_X,
        ROOT_Y,
        1400,
        420,
        0xff78_90a4,
        None,
    );
    assert_eq!(partial, reference);
}

#[test]
fn identity_only_generation_change_with_empty_damage_stays_empty() {
    let previous = test_renderable_surface(94, 180, 100, 500, 300, RenderableSurfaceDamage::Empty);
    let mut current = previous.clone();
    current.generation = previous.generation.saturating_add(1);
    let previous_scene = NativeSceneSnapshot::from_surfaces(&[previous], Vec::new());
    let current_scene = NativeSceneSnapshot::from_surfaces(&[current], Vec::new());

    let damage = native_output_damage_for_scene_snapshots(
        960,
        640,
        &previous_scene,
        &current_scene,
        NativeCursorDamageBounds::default(),
    );

    assert!(damage.is_empty());
}

#[test]
fn identity_only_commit_sequence_change_with_empty_damage_stays_empty() {
    let mut previous =
        test_renderable_surface(95, 180, 100, 500, 300, RenderableSurfaceDamage::Empty);
    previous.commit_sequence = SurfaceCommitSequence(40);
    let mut current = previous.clone();
    current.commit_sequence = SurfaceCommitSequence(41);
    let previous_scene = NativeSceneSnapshot::from_surfaces(&[previous], Vec::new());
    let current_scene = NativeSceneSnapshot::from_surfaces(&[current], Vec::new());

    let damage = native_output_damage_for_scene_snapshots(
        960,
        640,
        &previous_scene,
        &current_scene,
        NativeCursorDamageBounds::default(),
    );

    assert!(damage.is_empty());
}

#[test]
fn rejected_oversized_ssd_retry_matches_full_reference_for_buffer_ages_one_two_three() {
    const WIDTH: u32 = 1920;
    const HEIGHT: u32 = 1080;
    const ROOT_X: i32 = -200;
    const ROOT_Y: i32 = 160;
    const CLIENT_HEIGHT: u32 = 220;
    const TITLEBAR_HEIGHT: i32 = 26;
    const FRAME_BORDER: i32 = 1;
    let window_id = WindowId::from_raw(96).expect("non-zero test window id");
    let scene = |width: u32, generation: u64| {
        let mut surface = test_renderable_surface(
            96,
            ROOT_X - 72,
            ROOT_Y - 72,
            width,
            CLIENT_HEIGHT,
            RenderableSurfaceDamage::Empty,
        );
        surface.generation = generation;
        NativeSceneSnapshot::from_surfaces(
            std::slice::from_ref(&surface),
            vec![DecorationSceneSnapshot::from_bounds(
                window_id,
                96,
                ROOT_X - FRAME_BORDER,
                ROOT_Y - TITLEBAR_HEIGHT,
                width.saturating_add((FRAME_BORDER * 2) as u32),
                CLIENT_HEIGHT + TITLEBAR_HEIGHT as u32 + (FRAME_BORDER * 2) as u32,
                1,
            )],
        )
    };

    for age in 1..=3_u32 {
        let mut history = NativeSceneHistory::new(NativeFrameSceneSnapshot {
            frame_id: 1,
            render_generation: 1,
            scene: scene(2200, 100),
            cursor_damage: NativeCursorDamageBounds::default(),
        });
        let mut planner = PartialRepaintPlanner::new(
            (WIDTH, HEIGHT),
            EglPartialRepaintCapabilities {
                buffer_age: true,
                partial_render_repair: true,
                swap_buffers_with_damage: true,
            },
        );
        let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
        planner.commit_presented_transition(first.render_damage.clone());

        let intermediate_widths: &[u32] = match age {
            1 => &[],
            2 => &[2050],
            3 => &[2050, 1900],
            _ => unreachable!(),
        };
        for (index, width) in intermediate_widths.iter().copied().enumerate() {
            let current_scene = scene(width, 101 + index as u64);
            let current_damage = native_output_damage_for_scene_snapshots(
                WIDTH,
                HEIGHT,
                history.presented_scene(),
                &current_scene,
                NativeCursorDamageBounds::default(),
            )
            .as_renderer_damage(WIDTH, HEIGHT);
            let plan = planner.plan(current_damage, BufferAge::Value(1));
            assert_eq!(
                plan.mode,
                RepaintMode::Partial,
                "intermediate age history must remain partial at width {width}"
            );
            planner.commit_presented_transition(plan.render_damage.clone());
            let frame_id = 2 + index as u64;
            history.replace_ready(NativeFrameSceneSnapshot {
                frame_id,
                render_generation: frame_id,
                scene: current_scene,
                cursor_damage: NativeCursorDamageBounds::default(),
            });
            let token = 960 + frame_id;
            assert!(history.queue_submission(token));
            assert!(history.promote_pageflip(token));
        }

        let retry_scene = scene(1400, 110);
        let rejected_frame_id = 10 + age as u64;
        history.replace_ready(NativeFrameSceneSnapshot {
            frame_id: rejected_frame_id,
            render_generation: 900,
            scene: retry_scene.clone(),
            cursor_damage: NativeCursorDamageBounds::default(),
        });
        let rejected_token = 1000 + age as u64;
        assert!(history.queue_submission(rejected_token));
        assert!(history.discard_submission(rejected_token));
        history.replace_ready(NativeFrameSceneSnapshot {
            frame_id: rejected_frame_id + 1,
            render_generation: 900,
            scene: retry_scene.clone(),
            cursor_damage: NativeCursorDamageBounds::default(),
        });

        let current_damage = native_output_damage_for_scene_snapshots(
            WIDTH,
            HEIGHT,
            history.presented_scene(),
            &retry_scene,
            NativeCursorDamageBounds::default(),
        );
        let plan = planner.plan(
            current_damage.as_renderer_damage(WIDTH, HEIGHT),
            BufferAge::Value(age as i32),
        );
        assert_eq!(
            plan.mode,
            RepaintMode::Partial,
            "age {age} retry must use bounded partial repair"
        );
        let repair_rects = plan
            .repair_damage
            .rects_slice()
            .iter()
            .map(|rect| NativeDamageRect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            })
            .collect::<Vec<_>>();
        let mut partial = vec![0xff12_151c; (WIDTH * HEIGHT) as usize];
        paint_ssd_scene_with_height(
            &mut partial,
            WIDTH,
            HEIGHT,
            ROOT_X,
            ROOT_Y,
            2200,
            CLIENT_HEIGHT,
            TITLEBAR_HEIGHT,
            FRAME_BORDER,
            None,
        );
        paint_ssd_scene_with_height(
            &mut partial,
            WIDTH,
            HEIGHT,
            ROOT_X,
            ROOT_Y,
            1400,
            CLIENT_HEIGHT,
            TITLEBAR_HEIGHT,
            FRAME_BORDER,
            Some(&repair_rects),
        );
        let mut reference = vec![0xff12_151c; (WIDTH * HEIGHT) as usize];
        paint_ssd_scene_with_height(
            &mut reference,
            WIDTH,
            HEIGHT,
            ROOT_X,
            ROOT_Y,
            1400,
            CLIENT_HEIGHT,
            TITLEBAR_HEIGHT,
            FRAME_BORDER,
            None,
        );
        let mismatches = partial
            .iter()
            .zip(&reference)
            .filter(|(partial, reference)| partial != reference)
            .count();
        let first_mismatch = partial
            .iter()
            .zip(&reference)
            .position(|(partial, reference)| partial != reference)
            .map(|index| (index % WIDTH as usize, index / WIDTH as usize));
        assert_eq!(
            mismatches, 0,
            "age {age} retry differs from full B at {first_mismatch:?}; repair={repair_rects:?}"
        );
    }
}

#[test]
fn presented_scene_history_repairs_oversized_shrink_sequence() {
    const WIDTH: u32 = 1920;
    const HEIGHT: u32 = 1080;
    const TITLEBAR_HEIGHT: i32 = 26;
    const FRAME_BORDER: i32 = 1;
    const ROOT_X: i32 = -200;
    const ROOT_Y: i32 = 120;
    let widths: Vec<u32> = (0..=30).map(|step| 2200 - (step * 1400 / 30)).collect();
    let window_id = WindowId::from_raw(89).expect("non-zero test window id");
    let decoration = |width: u32| {
        DecorationSceneSnapshot::from_bounds(
            window_id,
            89,
            ROOT_X - FRAME_BORDER,
            ROOT_Y - TITLEBAR_HEIGHT,
            width.saturating_add((FRAME_BORDER * 2) as u32),
            420 + TITLEBAR_HEIGHT as u32 + (FRAME_BORDER * 2) as u32,
            1,
        )
    };
    let surface = |width: u32| {
        test_renderable_surface(
            89,
            ROOT_X - 72,
            ROOT_Y - 72,
            width,
            420,
            RenderableSurfaceDamage::Full,
        )
    };
    let scene = |surface: &RenderableSurface, width: u32| {
        NativeSceneSnapshot::from_surfaces(std::slice::from_ref(surface), vec![decoration(width)])
    };

    assert_eq!(widths.first(), Some(&2200));
    assert_eq!(widths.last(), Some(&800));
    let first_surface = surface(widths[0]);
    let mut history = NativeSceneHistory::new(NativeFrameSceneSnapshot {
        frame_id: 0,
        render_generation: 0,
        scene: scene(&first_surface, widths[0]),
        cursor_damage: NativeCursorDamageBounds::default(),
    });
    let mut partial = vec![0xff12_151c; (WIDTH * HEIGHT) as usize];
    paint_oversized_ssd_scene(
        &mut partial,
        WIDTH,
        HEIGHT,
        ROOT_X,
        ROOT_Y,
        widths[0],
        TITLEBAR_HEIGHT,
        FRAME_BORDER,
        None,
    );

    for (step, width) in widths.iter().copied().enumerate().skip(1) {
        let current_surface = surface(width);
        let current_scene = scene(&current_surface, width);
        let damage = native_output_damage_for_scene_snapshots(
            WIDTH,
            HEIGHT,
            history.presented_scene(),
            &current_scene,
            NativeCursorDamageBounds::default(),
        );
        paint_oversized_ssd_scene(
            &mut partial,
            WIDTH,
            HEIGHT,
            ROOT_X,
            ROOT_Y,
            width,
            TITLEBAR_HEIGHT,
            FRAME_BORDER,
            Some(&damage.rects),
        );
        let mut reference = vec![0xff12_151c; (WIDTH * HEIGHT) as usize];
        paint_oversized_ssd_scene(
            &mut reference,
            WIDTH,
            HEIGHT,
            ROOT_X,
            ROOT_Y,
            width,
            TITLEBAR_HEIGHT,
            FRAME_BORDER,
            None,
        );
        let mismatches = partial
            .iter()
            .zip(&reference)
            .filter(|(partial, reference)| partial != reference)
            .count();
        assert_eq!(mismatches, 0, "shrink step {step} at width {width}");

        history.replace_ready(NativeFrameSceneSnapshot {
            frame_id: step as u64,
            render_generation: step as u64,
            scene: current_scene,
            cursor_damage: NativeCursorDamageBounds::default(),
        });
        let token = 100 + step as u64;
        assert!(history.queue_submission(token));
        assert!(history.promote_pageflip(token));
    }
}

#[test]
fn presented_scene_damage_repairs_all_resize_edges() {
    const WIDTH: u32 = 1920;
    const HEIGHT: u32 = 1080;
    const TITLEBAR_HEIGHT: i32 = 26;
    const FRAME_BORDER: i32 = 1;
    let window_id = WindowId::from_raw(90).expect("non-zero test window id");
    let cases = [
        ("left", (100, 160, 1600, 800), (300, 160, 1400, 800)),
        ("right", (100, 160, 1600, 800), (100, 160, 1400, 800)),
        ("top", (100, 160, 1600, 800), (100, 280, 1600, 680)),
        ("bottom", (100, 160, 1600, 800), (100, 160, 1600, 680)),
        ("top-left", (100, 160, 1600, 800), (300, 280, 1400, 680)),
        ("top-right", (100, 160, 1600, 800), (100, 280, 1400, 680)),
        ("bottom-left", (100, 160, 1600, 800), (300, 160, 1400, 680)),
        ("bottom-right", (100, 160, 1600, 800), (100, 160, 1400, 680)),
    ];

    for (name, previous, current) in cases {
        let surface = |geometry: (i32, i32, u32, u32)| {
            test_renderable_surface(
                90,
                geometry.0 - 72,
                geometry.1 - 72,
                geometry.2,
                geometry.3,
                RenderableSurfaceDamage::Full,
            )
        };
        let decoration = |geometry: (i32, i32, u32, u32)| {
            DecorationSceneSnapshot::from_bounds(
                window_id,
                90,
                geometry.0 - FRAME_BORDER,
                geometry.1 - TITLEBAR_HEIGHT,
                geometry.2 + (FRAME_BORDER * 2) as u32,
                geometry.3 + TITLEBAR_HEIGHT as u32 + (FRAME_BORDER * 2) as u32,
                1,
            )
        };
        let previous_surface = surface(previous);
        let current_surface = surface(current);
        let previous_scene = NativeSceneSnapshot::from_surfaces(
            std::slice::from_ref(&previous_surface),
            vec![decoration(previous)],
        );
        let current_scene = NativeSceneSnapshot::from_surfaces(
            std::slice::from_ref(&current_surface),
            vec![decoration(current)],
        );
        let damage = native_output_damage_for_scene_snapshots(
            WIDTH,
            HEIGHT,
            &previous_scene,
            &current_scene,
            NativeCursorDamageBounds::default(),
        );

        assert!(
            damage
                .rects
                .iter()
                .any(|rect| native_damage_rect_contains(*rect, previous.0, previous.1)),
            "{name} resize must repair the previous visual bounds"
        );
        assert!(
            damage
                .rects
                .iter()
                .any(|rect| native_damage_rect_contains(*rect, current.0, current.1)),
            "{name} resize must repaint the current visual bounds"
        );
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "test raster helper keeps geometry and damage inputs explicit"
)]
fn paint_oversized_ssd_scene(
    frame: &mut [u32],
    width: u32,
    height: u32,
    root_x: i32,
    root_y: i32,
    client_width: u32,
    titlebar_height: i32,
    frame_border: i32,
    damage: Option<&[NativeDamageRect]>,
) {
    paint_ssd_scene_with_height(
        frame,
        width,
        height,
        root_x,
        root_y,
        client_width,
        420,
        titlebar_height,
        frame_border,
        damage,
    );
}

#[expect(
    clippy::too_many_arguments,
    reason = "test raster helper keeps geometry and damage inputs explicit"
)]
fn paint_ssd_scene_with_height(
    frame: &mut [u32],
    width: u32,
    height: u32,
    root_x: i32,
    root_y: i32,
    client_width: u32,
    client_height: u32,
    titlebar_height: i32,
    frame_border: i32,
    damage: Option<&[NativeDamageRect]>,
) {
    const BACKGROUND: u32 = 0xff12_151c;
    const TITLEBAR: u32 = 0xff33_3333;
    const CLIENT: u32 = 0xff65_7890;
    const BUTTON: u32 = 0xffd0_5040;
    let outer = NativeDamageRect {
        x: root_x - frame_border,
        y: root_y - titlebar_height,
        width: client_width.saturating_add((frame_border * 2) as u32),
        height: client_height + titlebar_height as u32 + (frame_border * 2) as u32,
    };
    let client = NativeDamageRect {
        x: root_x,
        y: root_y,
        width: client_width,
        height: client_height,
    };
    let buttons = NativeDamageRect {
        x: root_x + client_width as i32 - 180,
        y: root_y - titlebar_height + 8,
        width: 120,
        height: 10,
    };

    for y in 0..height as i32 {
        for x in 0..width as i32 {
            if damage.is_some_and(|rects| {
                !rects
                    .iter()
                    .any(|rect| native_damage_rect_contains(*rect, x, y))
            }) {
                continue;
            }
            let index = (y * width as i32 + x) as usize;
            frame[index] = if native_damage_rect_contains(buttons, x, y) {
                BUTTON
            } else if native_damage_rect_contains(client, x, y) {
                CLIENT
            } else if native_damage_rect_contains(outer, x, y) {
                TITLEBAR
            } else {
                BACKGROUND
            };
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "test raster helper keeps geometry and damage inputs explicit"
)]
fn paint_client_scene(
    frame: &mut [u32],
    width: u32,
    height: u32,
    root_x: i32,
    root_y: i32,
    client_width: u32,
    client_height: u32,
    client_color: u32,
    damage: Option<&[NativeDamageRect]>,
) {
    let client = NativeDamageRect {
        x: root_x,
        y: root_y,
        width: client_width,
        height: client_height,
    };
    for y in 0..height as i32 {
        for x in 0..width as i32 {
            if damage.is_some_and(|rects| {
                !rects
                    .iter()
                    .any(|rect| native_damage_rect_contains(*rect, x, y))
            }) {
                continue;
            }
            let index = (y * width as i32 + x) as usize;
            frame[index] = if native_damage_rect_contains(client, x, y) {
                client_color
            } else {
                0xff12_151c
            };
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "test raster helper keeps geometry and damage inputs explicit"
)]
fn paint_ssd_scene(
    frame: &mut [u32],
    width: u32,
    height: u32,
    position: (i32, i32),
    titlebar_height: i32,
    frame_border: i32,
    titlebar: u32,
    client: u32,
    damage: Option<&[NativeDamageRect]>,
) {
    let outer = NativeDamageRect {
        x: position.0 - frame_border,
        y: position.1 - titlebar_height,
        width: 300 + (frame_border * 2) as u32,
        height: 200 + titlebar_height as u32 + (frame_border * 2) as u32,
    };
    let client_rect = NativeDamageRect {
        x: position.0,
        y: position.1,
        width: 300,
        height: 200,
    };

    for y in 0..height as i32 {
        for x in 0..width as i32 {
            if damage.is_some_and(|rects| {
                !rects
                    .iter()
                    .any(|rect| native_damage_rect_contains(*rect, x, y))
            }) {
                continue;
            }
            let index = (y * width as i32 + x) as usize;
            frame[index] = if native_damage_rect_contains(client_rect, x, y) {
                client
            } else if native_damage_rect_contains(outer, x, y) {
                titlebar
            } else {
                0xff12_151c
            };
        }
    }
}

fn native_damage_rect_contains(rect: NativeDamageRect, x: i32, y: i32) -> bool {
    x >= rect.x
        && y >= rect.y
        && x < rect.x.saturating_add(rect.width as i32)
        && y < rect.y.saturating_add(rect.height as i32)
}

#[test]
fn native_output_damage_for_scene_change_survives_later_cursor_state_cause() {
    let previous = test_renderable_surface(8, 0, 0, 120, 80, RenderableSurfaceDamage::Empty);
    let current = test_renderable_surface(8, 200, 100, 120, 80, RenderableSurfaceDamage::Empty);
    let previous_origin = surface_origins(std::slice::from_ref(&previous))[0];
    let current_origin = surface_origins(std::slice::from_ref(&current))[0];

    let damage = native_output_damage_for_scene_and_cursor(
        400,
        300,
        std::slice::from_ref(&previous),
        std::slice::from_ref(&current),
        true,
        NativeCursorDamageBounds::default(),
    );

    assert_eq!(damage.kind, NativeDamageKind::SurfaceDamage);
    assert!(damage.rects.contains(&NativeDamageRect {
        x: previous_origin.0,
        y: previous_origin.1,
        width: 120,
        height: 80,
    }));
    assert!(damage.rects.contains(&NativeDamageRect {
        x: current_origin.0,
        y: current_origin.1,
        width: 120,
        height: 80,
    }));
}

#[test]
fn native_damage_accumulator_render_element_bounds_changes_cover_old_and_new_targets() {
    let previous = test_renderable_surface(7, 0, 0, 120, 80, RenderableSurfaceDamage::Full);
    let current = test_renderable_surface(7, 200, 100, 120, 80, RenderableSurfaceDamage::Full);
    let previous_elements =
        render_scene_elements_for_surfaces(std::slice::from_ref(&previous), 1.0);
    let current_elements = render_scene_elements_for_surfaces(std::slice::from_ref(&current), 1.0);

    let damage = NativeDamageAccumulator::from_render_element_bounds_changes(
        400,
        300,
        &previous_elements,
        &current_elements,
    );

    assert_eq!(
        damage.rects(),
        &[
            NativeDamageRect {
                x: 72,
                y: 72,
                width: 120,
                height: 80,
            },
            NativeDamageRect {
                x: 272,
                y: 172,
                width: 120,
                height: 80,
            },
        ]
    );
}

#[test]
fn native_output_damage_for_window_resize_covers_rescaled_bounds() {
    let previous = test_renderable_surface(7, 0, 0, 300, 200, RenderableSurfaceDamage::Full);
    let current = RenderableSurface {
        render_target_size: Some(BufferSize::new(340, 230).unwrap()),
        render_placement: None,
        visual_clip: None,
        ..test_renderable_surface(7, 0, 0, 300, 200, RenderableSurfaceDamage::Full)
    };
    let origin = surface_origins(std::slice::from_ref(&previous))[0];

    let damage = native_output_damage_for_repaint(
        640,
        480,
        std::slice::from_ref(&previous),
        std::slice::from_ref(&current),
        RenderGenerationCause::WindowResize,
        true,
    );

    assert_eq!(damage.kind, NativeDamageKind::SurfaceDamage);
    assert_eq!(
        damage.rects,
        vec![NativeDamageRect {
            x: origin.0,
            y: origin.1,
            width: 340,
            height: 230,
        }]
    );
}

#[test]
fn task_05_8_native_damage_for_window_resize_covers_visual_clip_changes() {
    let origin = surface_origins(&[test_renderable_surface(
        7,
        0,
        0,
        300,
        200,
        RenderableSurfaceDamage::Full,
    )])[0];
    let previous = RenderableSurface {
        visual_clip: Some(
            oblivion_one::compositor::SurfaceVisualAperture::logical_only(
                oblivion_one::compositor::SurfaceTargetRect::new(0, 0, 300, 200),
            ),
        ),
        ..test_renderable_surface(7, 0, 0, 300, 200, RenderableSurfaceDamage::Full)
    };
    let current = RenderableSurface {
        visual_clip: Some(
            oblivion_one::compositor::SurfaceVisualAperture::logical_only(
                oblivion_one::compositor::SurfaceTargetRect::new(0, 0, 220, 160),
            ),
        ),
        generation: 1,
        ..test_renderable_surface(7, 0, 0, 300, 200, RenderableSurfaceDamage::Full)
    };

    let damage = native_output_damage_for_repaint(
        640,
        480,
        std::slice::from_ref(&previous),
        std::slice::from_ref(&current),
        RenderGenerationCause::WindowResize,
        true,
    );

    assert_eq!(damage.kind, NativeDamageKind::SurfaceDamage);
    assert_eq!(
        damage.rects,
        vec![NativeDamageRect {
            x: origin.0,
            y: origin.1,
            width: 300,
            height: 200,
        }]
    );
}

#[test]
fn task_3_native_damage_covers_old_and_new_root_aperture_bounds() {
    let previous_aperture =
        oblivion_one::compositor::SurfaceVisualAperture::for_root_window_preview(
            (100, 100),
            BufferSize::new(332, 242).expect("root buffer"),
            (16, 10, 16, 32),
            oblivion_one::compositor::SurfaceTargetRect::new(116, 110, 300, 200),
        );
    let current_aperture = oblivion_one::compositor::SurfaceVisualAperture::for_root_window_preview(
        (500, 400),
        BufferSize::new(332, 242).expect("root buffer"),
        (16, 10, 16, 32),
        oblivion_one::compositor::SurfaceTargetRect::new(516, 410, 300, 200),
    );
    let previous = RenderableSurface {
        placement: SurfacePlacement::absolute_root_at(100, 100),
        render_placement: Some(SurfacePlacement::absolute_root_at(100, 100)),
        visual_clip: Some(previous_aperture),
        ..test_renderable_surface(7, 0, 0, 332, 242, RenderableSurfaceDamage::Full)
    };
    let current = RenderableSurface {
        placement: SurfacePlacement::absolute_root_at(500, 400),
        render_placement: Some(SurfacePlacement::absolute_root_at(500, 400)),
        visual_clip: Some(current_aperture),
        generation: 2,
        ..previous.clone()
    };
    let previous_bounds = render_scene_elements_for_surfaces(std::slice::from_ref(&previous), 1.0)
        .pop()
        .expect("previous root element")
        .backing_target()
        .expect("previous root bounds");
    let current_bounds = render_scene_elements_for_surfaces(std::slice::from_ref(&current), 1.0)
        .pop()
        .expect("current root element")
        .backing_target()
        .expect("current root bounds");

    let damage = native_output_damage_for_repaint(
        1000,
        800,
        std::slice::from_ref(&previous),
        std::slice::from_ref(&current),
        RenderGenerationCause::WindowResize,
        true,
    );

    assert!(damage.rects.contains(&NativeDamageRect {
        x: previous_bounds.x(),
        y: previous_bounds.y(),
        width: previous_bounds.width(),
        height: previous_bounds.height(),
    }));
    assert!(damage.rects.contains(&NativeDamageRect {
        x: current_bounds.x(),
        y: current_bounds.y(),
        width: current_bounds.width(),
        height: current_bounds.height(),
    }));
}

#[test]
fn native_output_damage_for_surface_commit_bounds_change_covers_old_and_new_bounds() {
    let previous = test_renderable_surface(7, 0, 0, 300, 200, RenderableSurfaceDamage::Full);
    let current = RenderableSurface {
        width: 260,
        height: 200,
        placement: SurfacePlacement::root_at(40, 0),
        damage: RenderableSurfaceDamage::Full,
        ..test_renderable_surface(7, 0, 0, 300, 200, RenderableSurfaceDamage::Full)
    };
    let previous_origin = surface_origins(std::slice::from_ref(&previous))[0];

    let damage = native_output_damage_for_repaint(
        640,
        480,
        std::slice::from_ref(&previous),
        std::slice::from_ref(&current),
        RenderGenerationCause::SurfaceCommit,
        true,
    );

    assert_eq!(damage.kind, NativeDamageKind::SurfaceDamage);
    assert_eq!(
        damage.rects,
        vec![NativeDamageRect {
            x: previous_origin.0,
            y: previous_origin.1,
            width: 300,
            height: 200,
        }]
    );
}

#[test]
fn native_output_damage_forces_full_copy_after_full_scene_rebuild() {
    let rects = [NativeDamageRect {
        x: 10,
        y: 12,
        width: 20,
        height: 24,
    }];
    let damage = NativeOutputDamage::surface_damage(rects.to_vec());

    assert!(matches!(
        damage.frame_copy_damage_for_scene(DesktopSceneRebuildKind::Full),
        NativeFrameCopyDamage::Full
    ));
    assert!(matches!(
        damage.frame_copy_damage_for_scene(DesktopSceneRebuildKind::Partial),
        NativeFrameCopyDamage::Rects(partial) if partial == rects
    ));
}

pub(super) fn test_renderable_surface(
    surface_id: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    damage: RenderableSurfaceDamage,
) -> RenderableSurface {
    let size = BufferSize::new(width, height).expect("test surface size must be non-zero");
    RenderableSurface {
        surface_id,
        x,
        y,
        width,
        height,
        placement: SurfacePlacement::root(),
        render_backend: oblivion_one::compositor::SurfaceRenderBackend::Xwayland,
        render_placement: None,
        visual_clip: None,
        render_target_size: None,
        commit_sequence: SurfaceCommitSequence::initial(),
        generation: 0,
        buffer: CommittedSurfaceBuffer::shm_snapshot(
            test_buffer_identity(),
            size,
            vec![0; width as usize * height as usize],
        ),
        viewport_source: None,
        viewport_destination: None,
        buffer_scale: 1,
        buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
        opaque_region: oblivion_one::compositor::SurfaceOpaqueRegion::None,
        damage,
    }
}

#[test]
fn stable_geometry_content_damage_does_not_escalate_to_full_bounds() {
    const WIDTH: u32 = 800;
    const HEIGHT: u32 = 600;
    let mut previous =
        test_renderable_surface(197, 100, 80, 400, 300, RenderableSurfaceDamage::Empty);
    previous.generation = 10;
    previous.commit_sequence = SurfaceCommitSequence(20);
    let mut current = previous.clone();
    current.generation = 11;
    current.commit_sequence = SurfaceCommitSequence(21);
    current.damage = RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
        x: 10,
        y: 20,
        width: 5,
        height: 6,
    }]);

    let previous_scene = NativeSceneSnapshot::from_surfaces(&[previous], Vec::new());
    let current_scene = NativeSceneSnapshot::from_surfaces(&[current], Vec::new());
    let damage = native_output_damage_for_scene_snapshots(
        WIDTH,
        HEIGHT,
        &previous_scene,
        &current_scene,
        NativeCursorDamageBounds::default(),
    );

    assert_eq!(damage.rects.len(), 1);
    assert_eq!(damage.rects[0].width, 5);
    assert_eq!(damage.rects[0].height, 6);
}

#[test]
fn popup_map_is_regional_and_does_not_damage_unrelated_output() {
    let base = test_renderable_surface(300, 0, 0, 960, 640, RenderableSurfaceDamage::Empty);
    let popup = test_renderable_surface(301, 320, 220, 80, 60, RenderableSurfaceDamage::Full);
    let previous = NativeSceneSnapshot::from_surfaces(std::slice::from_ref(&base), Vec::new());
    let mut current = NativeSceneSnapshot::from_surfaces(&[base, popup], Vec::new());
    current.popup_surface_ids = vec![301];

    let damage = native_output_damage_for_scene_snapshots(
        960,
        640,
        &previous,
        &current,
        NativeCursorDamageBounds::default(),
    );

    assert_ne!(damage.kind, NativeDamageKind::FullOutput);
    assert!(
        damage
            .rects
            .iter()
            .any(|rect| native_damage_rect_contains(*rect, 430, 330)),
        "popup map damage: {:?}",
        damage
    );
    assert!(
        !damage
            .rects
            .iter()
            .any(|rect| native_damage_rect_contains(*rect, 900, 600))
    );
}

#[test]
fn popup_unmap_is_regional_and_repairs_the_old_tree_footprint() {
    let base = test_renderable_surface(310, 0, 0, 960, 640, RenderableSurfaceDamage::Empty);
    let popup = test_renderable_surface(311, 320, 220, 80, 60, RenderableSurfaceDamage::Empty);
    let mut previous = NativeSceneSnapshot::from_surfaces(&[base.clone(), popup], Vec::new());
    previous.popup_surface_ids = vec![311];
    let current = NativeSceneSnapshot::from_surfaces(std::slice::from_ref(&base), Vec::new());

    let damage = native_output_damage_for_scene_snapshots(
        960,
        640,
        &previous,
        &current,
        NativeCursorDamageBounds::default(),
    );

    assert_ne!(damage.kind, NativeDamageKind::FullOutput);
    assert!(
        damage
            .rects
            .iter()
            .any(|rect| native_damage_rect_contains(*rect, 430, 330)),
        "popup unmap damage: {:?}",
        damage
    );
    assert!(
        !damage
            .rects
            .iter()
            .any(|rect| native_damage_rect_contains(*rect, 900, 600))
    );
}

#[test]
fn popup_reposition_repairs_old_and_new_regions_without_full_output() {
    let previous_popup =
        test_renderable_surface(321, 120, 100, 80, 60, RenderableSurfaceDamage::Empty);
    let current_popup =
        test_renderable_surface(321, 500, 400, 80, 60, RenderableSurfaceDamage::Empty);
    let previous = NativeSceneSnapshot::from_surfaces(&[previous_popup], Vec::new());
    let current = NativeSceneSnapshot::from_surfaces(&[current_popup], Vec::new());

    let damage = native_output_damage_for_scene_snapshots(
        960,
        640,
        &previous,
        &current,
        NativeCursorDamageBounds::default(),
    );

    assert_ne!(damage.kind, NativeDamageKind::FullOutput);
    assert!(
        damage
            .rects
            .iter()
            .any(|rect| native_damage_rect_contains(*rect, 200, 180))
    );
    assert!(
        damage
            .rects
            .iter()
            .any(|rect| native_damage_rect_contains(*rect, 580, 480))
    );
}

#[test]
fn popup_content_only_commit_remains_regional() {
    let previous_popup =
        test_renderable_surface(331, 120, 100, 120, 90, RenderableSurfaceDamage::Empty);
    let mut current_popup = previous_popup.clone();
    current_popup.generation = 2;
    current_popup.commit_sequence = SurfaceCommitSequence(2);
    current_popup.damage = RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
        x: 4,
        y: 5,
        width: 8,
        height: 9,
    }]);
    let previous = NativeSceneSnapshot::from_surfaces(&[previous_popup], Vec::new());
    let current = NativeSceneSnapshot::from_surfaces(&[current_popup], Vec::new());

    let damage = native_output_damage_for_scene_snapshots(
        960,
        640,
        &previous,
        &current,
        NativeCursorDamageBounds::default(),
    );

    assert_ne!(damage.kind, NativeDamageKind::FullOutput);
    assert_eq!(damage.rects.len(), 1);
    assert!(native_damage_rect_contains(damage.rects[0], 196, 177));
    assert!(!native_damage_rect_contains(damage.rects[0], 260, 220));
}

#[test]
fn subsurface_map_unmap_and_move_repair_only_child_regions() {
    let parent = test_renderable_surface(340, 100, 100, 300, 220, RenderableSurfaceDamage::Empty);
    let mut old_child = test_renderable_surface(341, 0, 0, 40, 30, RenderableSurfaceDamage::Empty);
    old_child.placement = SurfacePlacement::subsurface(340, 40, 40);
    let mut new_child =
        test_renderable_surface(341, 120, 40, 40, 30, RenderableSurfaceDamage::Empty);
    new_child.placement = SurfacePlacement::subsurface(340, 40, 40);
    let previous = NativeSceneSnapshot::from_surfaces(&[parent.clone(), old_child], Vec::new());
    let current = NativeSceneSnapshot::from_surfaces(&[parent, new_child], Vec::new());

    let damage = native_output_damage_for_scene_snapshots(
        960,
        640,
        &previous,
        &current,
        NativeCursorDamageBounds::default(),
    );

    assert_ne!(damage.kind, NativeDamageKind::FullOutput);
    assert!(
        damage
            .rects
            .iter()
            .any(|rect| native_damage_rect_contains(*rect, 215, 215))
    );
    assert!(
        damage
            .rects
            .iter()
            .any(|rect| native_damage_rect_contains(*rect, 335, 255))
    );
    assert!(
        !damage
            .rects
            .iter()
            .any(|rect| native_damage_rect_contains(*rect, 900, 600))
    );
}

#[test]
fn subsurface_reorder_damages_only_the_changed_middle_span() {
    let surface = |surface_id, x| {
        test_renderable_surface(surface_id, x, 120, 40, 40, RenderableSurfaceDamage::Empty)
    };
    let a = surface(350, 20);
    let b = surface(351, 100);
    let c = surface(352, 180);
    let d = surface(353, 260);
    let e = surface(354, 340);
    let previous = NativeSceneSnapshot::from_surfaces(
        &[a.clone(), b.clone(), c.clone(), d.clone(), e.clone()],
        Vec::new(),
    );
    let current = NativeSceneSnapshot::from_surfaces(&[a, b, e, c, d], Vec::new());

    let damage = native_output_damage_for_scene_snapshots(
        960,
        640,
        &previous,
        &current,
        NativeCursorDamageBounds::default(),
    );

    assert_ne!(damage.kind, NativeDamageKind::FullOutput);
    for (x, y) in [(320, 260), (440, 290), (550, 330), (350, 260), (470, 330)] {
        assert!(
            damage
                .rects
                .iter()
                .any(|rect| native_damage_rect_contains(*rect, x, y)),
            "changed order span must cover ({x}, {y}): {:?}",
            damage.rects
        );
    }
    assert!(
        !damage
            .rects
            .iter()
            .any(|rect| native_damage_rect_contains(*rect, 100, 200))
    );
}

#[test]
fn decorated_window_reorder_repairs_ssd_only_pixels_regionally() {
    let window_a = test_renderable_surface(380, 100, 100, 200, 200, RenderableSurfaceDamage::Empty);
    let window_b = test_renderable_surface(381, 180, 120, 200, 200, RenderableSurfaceDamage::Empty);
    let decoration_a = DecorationSceneSnapshot::from_bounds(
        WindowId::from_raw(380).expect("window A id"),
        380,
        96,
        70,
        208,
        234,
        1,
    );
    let decoration_b = DecorationSceneSnapshot::from_bounds(
        WindowId::from_raw(381).expect("window B id"),
        381,
        176,
        90,
        208,
        234,
        1,
    );
    let previous = NativeSceneSnapshot::from_surfaces(
        &[window_a.clone(), window_b.clone()],
        vec![decoration_a.clone(), decoration_b.clone()],
    );
    let current =
        NativeSceneSnapshot::from_surfaces(&[window_b, window_a], vec![decoration_b, decoration_a]);

    let damage = native_output_damage_for_scene_snapshots(
        960,
        640,
        &previous,
        &current,
        NativeCursorDamageBounds::default(),
    );

    assert_ne!(damage.kind, NativeDamageKind::FullOutput);
    assert!(
        damage
            .rects
            .iter()
            .any(|rect| native_damage_rect_contains(*rect, 190, 95)),
        "reorder must repair overlapping SSD-only pixels: {:?}",
        damage.rects
    );
    assert!(
        !damage
            .rects
            .iter()
            .any(|rect| native_damage_rect_contains(*rect, 900, 600))
    );
}

#[test]
fn unchanged_decorated_window_order_does_not_damage_ssd() {
    let window_a = test_renderable_surface(390, 100, 100, 200, 200, RenderableSurfaceDamage::Empty);
    let window_b = test_renderable_surface(391, 180, 120, 200, 200, RenderableSurfaceDamage::Empty);
    let decorations = vec![
        DecorationSceneSnapshot::from_bounds(
            WindowId::from_raw(390).expect("window A id"),
            390,
            96,
            70,
            208,
            234,
            1,
        ),
        DecorationSceneSnapshot::from_bounds(
            WindowId::from_raw(391).expect("window B id"),
            391,
            176,
            90,
            208,
            234,
            1,
        ),
    ];
    let previous = NativeSceneSnapshot::from_surfaces(
        &[window_a.clone(), window_b.clone()],
        decorations.clone(),
    );
    let current = NativeSceneSnapshot::from_surfaces(&[window_a, window_b], decorations);

    let damage = native_output_damage_for_scene_snapshots(
        960,
        640,
        &previous,
        &current,
        NativeCursorDamageBounds::default(),
    );

    assert!(
        damage.is_empty(),
        "unchanged order must not repaint SSDs: {damage:?}"
    );
}

#[test]
fn visibility_transition_remains_a_true_global_invalidation() {
    let surface = test_renderable_surface(360, 100, 100, 80, 60, RenderableSurfaceDamage::Empty);
    let previous = NativeSceneSnapshot::from_surfaces(std::slice::from_ref(&surface), Vec::new());
    let mut current = previous.clone();
    current.visibility_signature = 1;

    let damage = native_output_damage_for_scene_snapshots(
        960,
        640,
        &previous,
        &current,
        NativeCursorDamageBounds::default(),
    );

    assert_eq!(damage.kind, NativeDamageKind::FullOutput);
}

#[test]
fn authoritative_empty_remains_empty_when_content_identity_changes() {
    let previous = test_renderable_surface(370, 100, 100, 120, 90, RenderableSurfaceDamage::Empty);
    let mut current = previous.clone();
    current.generation = 2;
    current.commit_sequence = SurfaceCommitSequence(2);
    current.damage = RenderableSurfaceDamage::Empty;
    let previous = NativeSceneSnapshot::from_surfaces(&[previous], Vec::new());
    let current = NativeSceneSnapshot::from_surfaces(&[current], Vec::new());

    let damage = native_output_damage_for_scene_snapshots(
        960,
        640,
        &previous,
        &current,
        NativeCursorDamageBounds::default(),
    );

    assert!(damage.is_empty());
}

#[test]
fn native_snapshot_exposes_authoritative_empty_damage_evidence() {
    let surface = test_renderable_surface(371, 100, 100, 120, 90, RenderableSurfaceDamage::Empty);
    let snapshot = NativeSceneSnapshot::from_surfaces(&[surface], Vec::new());

    assert_eq!(
        snapshot.surfaces[0].damage,
        NativeSurfaceDamageEvidence::AuthoritativeEmpty
    );
}

#[test]
fn native_snapshot_exposes_history_lost_damage_evidence() {
    let surface =
        test_renderable_surface(372, 100, 100, 120, 90, RenderableSurfaceDamage::HistoryLost);
    let snapshot = NativeSceneSnapshot::from_surfaces(&[surface], Vec::new());

    assert_eq!(
        snapshot.surfaces[0].damage,
        NativeSurfaceDamageEvidence::HistoryLost
    );
}

#[test]
fn history_lost_repairs_only_the_current_surface_footprint() {
    let previous_surface =
        test_renderable_surface(373, 100, 100, 120, 90, RenderableSurfaceDamage::Empty);
    let current_surface =
        test_renderable_surface(373, 100, 100, 120, 90, RenderableSurfaceDamage::HistoryLost);
    let previous = NativeSceneSnapshot::from_surfaces(&[previous_surface], Vec::new());
    let current = NativeSceneSnapshot::from_surfaces(&[current_surface], Vec::new());

    let damage = native_output_damage_for_scene_snapshots(
        960,
        640,
        &previous,
        &current,
        NativeCursorDamageBounds::default(),
    );

    assert_ne!(damage.kind, NativeDamageKind::FullOutput);
    assert!(
        damage
            .rects
            .iter()
            .any(|rect| native_damage_rect_contains(*rect, 200, 180))
    );
    assert!(
        !damage
            .rects
            .iter()
            .any(|rect| native_damage_rect_contains(*rect, 900, 600))
    );
}

#[derive(Clone)]
struct TopologyOracleVisuals {
    colors: std::collections::HashMap<u32, u32>,
    decoration_colors: std::collections::HashMap<u32, u32>,
    markers: std::collections::HashMap<u32, (u32, u32, u32, u32, u32)>,
}

fn topology_oracle_surface(
    surface_id: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    damage: RenderableSurfaceDamage,
) -> RenderableSurface {
    let mut surface = test_renderable_surface(surface_id, 0, 0, width, height, damage);
    surface.placement = SurfacePlacement::absolute_root_at(x, y);
    surface
}

fn topology_oracle_surface_with_buffer(
    surface_id: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    buffer: BufferIdentity,
    damage: RenderableSurfaceDamage,
) -> RenderableSurface {
    let mut surface = topology_oracle_surface(surface_id, x, y, width, height, damage);
    surface.buffer = CommittedSurfaceBuffer::shm_snapshot(
        buffer,
        BufferSize::new(width, height).expect("test surface size must be non-zero"),
        vec![0; width as usize * height as usize],
    );
    surface
}

fn topology_oracle_snapshot(
    surfaces: &[RenderableSurface],
    popup_surface_ids: &[u32],
) -> NativeSceneSnapshot {
    let decorations = surfaces
        .iter()
        .filter_map(|surface| match surface.surface_id {
            401 => Some(DecorationSceneSnapshot::from_bounds(
                WindowId::from_raw(401).expect("window A id"),
                401,
                16,
                0,
                168,
                148,
                1,
            )),
            402 => Some(DecorationSceneSnapshot::from_bounds(
                WindowId::from_raw(402).expect("window B id"),
                402,
                86,
                50,
                168,
                148,
                1,
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    NativeSceneSnapshot::from_surfaces_with_popup_ids(surfaces, decorations, popup_surface_ids)
}

fn topology_oracle_pixel(
    snapshot: &NativeSceneSnapshot,
    visuals: &TopologyOracleVisuals,
    x: i32,
    y: i32,
) -> u32 {
    let mut pixel = 0xff00_0000;
    let mut painted_roots = std::collections::HashSet::new();
    for surface in &snapshot.surfaces {
        let root_surface_id = surface.visual_root_surface_id;
        if !painted_roots.insert(root_surface_id) {
            continue;
        }
        for member in snapshot
            .surfaces
            .iter()
            .filter(|member| member.visual_root_surface_id == root_surface_id)
        {
            let Some(bounds) = member.bounds else {
                continue;
            };
            if !native_damage_rect_contains(bounds, x, y) {
                continue;
            }
            pixel = visuals.colors[&member.surface_id];
            if let Some((marker_x, marker_y, marker_width, marker_height, marker_color)) =
                visuals.markers.get(&member.surface_id).copied()
            {
                let marker = NativeDamageRect {
                    x: bounds.x.saturating_add(marker_x as i32),
                    y: bounds.y.saturating_add(marker_y as i32),
                    width: marker_width,
                    height: marker_height,
                };
                if native_damage_rect_contains(marker, x, y) {
                    pixel = marker_color;
                }
            }
        }
        if !snapshot.popup_surface_ids.contains(&root_surface_id)
            && let Some(decoration) = snapshot
                .decorations
                .iter()
                .find(|decoration| decoration.identity().1 == root_surface_id)
        {
            let (decoration_x, decoration_y, decoration_width, decoration_height) =
                decoration.bounds();
            let bounds = NativeDamageRect {
                x: decoration_x,
                y: decoration_y,
                width: decoration_width,
                height: decoration_height,
            };
            if native_damage_rect_contains(bounds, x, y) {
                pixel = visuals.decoration_colors[&root_surface_id];
            }
        }
    }
    pixel
}

fn topology_oracle_paint_repair(
    frame: &mut [u32],
    width: u32,
    height: u32,
    snapshot: &NativeSceneSnapshot,
    visuals: &TopologyOracleVisuals,
    repair_damage: &OutputDamage,
) {
    for y in 0..height as i32 {
        for x in 0..width as i32 {
            let damaged = match repair_damage {
                OutputDamage::Full => true,
                OutputDamage::Empty => false,
                OutputDamage::Rects(rects) => rects.iter().any(|rect| {
                    native_damage_rect_contains(
                        NativeDamageRect {
                            x: rect.x,
                            y: rect.y,
                            width: rect.width,
                            height: rect.height,
                        },
                        x,
                        y,
                    )
                }),
            };
            if damaged {
                frame[(y as u32 * width + x as u32) as usize] =
                    topology_oracle_pixel(snapshot, visuals, x, y);
            }
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the oracle keeps each output state explicit"
)]
fn topology_oracle_present(
    planner: &mut PartialRepaintPlanner,
    slots: &mut [Vec<u32>],
    slot_index: usize,
    age: i32,
    presented_scene: &mut NativeSceneSnapshot,
    current_scene: NativeSceneSnapshot,
    visuals: &TopologyOracleVisuals,
    width: u32,
    height: u32,
    label: &str,
) {
    let current_damage = native_output_damage_for_scene_snapshots(
        width,
        height,
        presented_scene,
        &current_scene,
        NativeCursorDamageBounds::default(),
    );
    let plan = planner.plan(
        current_damage.as_renderer_damage(width, height),
        BufferAge::Value(age),
    );
    topology_oracle_paint_repair(
        &mut slots[slot_index],
        width,
        height,
        &current_scene,
        visuals,
        &plan.repair_damage,
    );
    let reference = (0..height as i32)
        .flat_map(|y| (0..width as i32).map(move |x| (x, y)))
        .map(|(x, y)| topology_oracle_pixel(&current_scene, visuals, x, y))
        .collect::<Vec<_>>();
    let first_mismatch = slots[slot_index]
        .iter()
        .zip(&reference)
        .position(|(actual, expected)| actual != expected)
        .map(|index| (index % width as usize, index / width as usize));
    assert_eq!(
        slots[slot_index], reference,
        "regional topology repaint differs from full reference after {label} at {first_mismatch:?}; logical={current_damage:?} repair={:?}",
        plan.repair_damage
    );
    planner.commit_presented_transition(plan.render_damage);
    *presented_scene = current_scene;
}

#[expect(
    clippy::too_many_arguments,
    reason = "the oracle keeps each rejected output state explicit"
)]
fn topology_oracle_reject_candidate(
    planner: &mut PartialRepaintPlanner,
    slots: &mut [Vec<u32>],
    slot_index: usize,
    age: i32,
    presented_scene: &NativeSceneSnapshot,
    current_scene: &NativeSceneSnapshot,
    visuals: &TopologyOracleVisuals,
    width: u32,
    height: u32,
    label: &str,
) {
    let current_damage = native_output_damage_for_scene_snapshots(
        width,
        height,
        presented_scene,
        current_scene,
        NativeCursorDamageBounds::default(),
    );
    let plan = planner.plan(
        current_damage.as_renderer_damage(width, height),
        BufferAge::Value(age),
    );
    topology_oracle_paint_repair(
        &mut slots[slot_index],
        width,
        height,
        current_scene,
        visuals,
        &plan.repair_damage,
    );
    let reference = (0..height as i32)
        .flat_map(|y| (0..width as i32).map(move |x| (x, y)))
        .map(|(x, y)| topology_oracle_pixel(current_scene, visuals, x, y))
        .collect::<Vec<_>>();
    assert_eq!(
        slots[slot_index], reference,
        "rejected regional topology candidate differs from full reference after {label}"
    );
    // A rejected render may have written a scratch/output slot, but it is not
    // a presented transition: neither planner history nor the presented scene
    // is advanced here. The next selected slot must repair from that state.
}

#[test]
fn topology_transitions_match_full_reference_with_rotating_output_ages() {
    const WIDTH: u32 = 640;
    const HEIGHT: u32 = 480;
    let client_buffer_a = test_buffer_identity();
    let client_buffer_b = test_buffer_identity();
    let client_buffer_c = test_buffer_identity();
    let a = topology_oracle_surface_with_buffer(
        401,
        20,
        20,
        160,
        120,
        client_buffer_a.clone(),
        RenderableSurfaceDamage::Empty,
    );
    let b = topology_oracle_surface_with_buffer(
        402,
        90,
        70,
        160,
        120,
        client_buffer_a.clone(),
        RenderableSurfaceDamage::Empty,
    );
    let mut visuals = TopologyOracleVisuals {
        colors: [
            (401, 0xff00_55aa),
            (402, 0xff22_aa55),
            (403, 0xffaa_5522),
            (404, 0xffaa_22aa),
            (405, 0xff55_aaaa),
        ]
        .into_iter()
        .collect(),
        decoration_colors: [(401, 0xff11_3355), (402, 0xff55_3311)]
            .into_iter()
            .collect(),
        markers: std::collections::HashMap::new(),
    };
    let initial_scene = topology_oracle_snapshot(&[a.clone(), b.clone()], &[]);
    let initial = (0..HEIGHT as i32)
        .flat_map(|y| (0..WIDTH as i32).map(move |x| (x, y)))
        .map(|(x, y)| topology_oracle_pixel(&initial_scene, &visuals, x, y))
        .collect::<Vec<_>>();
    let mut slots = vec![initial.clone(), initial.clone(), initial];
    let mut planner = PartialRepaintPlanner::new(
        (WIDTH, HEIGHT),
        EglPartialRepaintCapabilities {
            buffer_age: true,
            partial_render_repair: true,
            swap_buffers_with_damage: true,
        },
    );
    let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
    planner.commit_presented_transition(first.render_damage);
    let mut presented_scene = initial_scene;

    let mut b_content = b.clone();
    b_content.generation = 2;
    b_content.commit_sequence = SurfaceCommitSequence(2);
    b_content.damage = RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
        x: 12,
        y: 14,
        width: 32,
        height: 24,
    }]);
    visuals.markers.insert(402, (12, 14, 32, 24, 0xffff_ee22));
    topology_oracle_present(
        &mut planner,
        &mut slots,
        1,
        1,
        &mut presented_scene,
        topology_oracle_snapshot(&[a.clone(), b_content.clone()], &[]),
        &visuals,
        WIDTH,
        HEIGHT,
        "content partial damage",
    );

    let popup = topology_oracle_surface(403, 130, 100, 110, 80, RenderableSurfaceDamage::Full);
    topology_oracle_present(
        &mut planner,
        &mut slots,
        2,
        1,
        &mut presented_scene,
        topology_oracle_snapshot(&[a.clone(), b_content.clone(), popup.clone()], &[403]),
        &visuals,
        WIDTH,
        HEIGHT,
        "popup map",
    );

    let moved_popup =
        topology_oracle_surface(403, 360, 260, 110, 80, RenderableSurfaceDamage::Empty);
    topology_oracle_present(
        &mut planner,
        &mut slots,
        0,
        1,
        &mut presented_scene,
        topology_oracle_snapshot(&[a.clone(), b_content.clone(), moved_popup.clone()], &[403]),
        &visuals,
        WIDTH,
        HEIGHT,
        "popup move",
    );

    topology_oracle_present(
        &mut planner,
        &mut slots,
        1,
        3,
        &mut presented_scene,
        topology_oracle_snapshot(&[a.clone(), moved_popup.clone(), b_content.clone()], &[403]),
        &visuals,
        WIDTH,
        HEIGHT,
        "popup reorder",
    );

    topology_oracle_present(
        &mut planner,
        &mut slots,
        2,
        3,
        &mut presented_scene,
        topology_oracle_snapshot(&[a.clone(), b_content.clone()], &[]),
        &visuals,
        WIDTH,
        HEIGHT,
        "popup unmap",
    );

    let mut child = topology_oracle_surface(404, 0, 0, 70, 50, RenderableSurfaceDamage::Full);
    child.placement = SurfacePlacement::subsurface(401, 35, 30);
    topology_oracle_present(
        &mut planner,
        &mut slots,
        0,
        3,
        &mut presented_scene,
        topology_oracle_snapshot(&[a.clone(), b_content.clone(), child.clone()], &[]),
        &visuals,
        WIDTH,
        HEIGHT,
        "subsurface map",
    );

    let mut moved_child = child.clone();
    moved_child.placement = SurfacePlacement::subsurface(401, 95, 55);
    moved_child.damage = RenderableSurfaceDamage::Empty;
    topology_oracle_present(
        &mut planner,
        &mut slots,
        1,
        3,
        &mut presented_scene,
        topology_oracle_snapshot(&[a.clone(), b_content.clone(), moved_child.clone()], &[]),
        &visuals,
        WIDTH,
        HEIGHT,
        "subsurface move",
    );

    let mut sibling = topology_oracle_surface(405, 0, 0, 80, 60, RenderableSurfaceDamage::Full);
    sibling.placement = SurfacePlacement::subsurface(401, 65, 45);
    topology_oracle_present(
        &mut planner,
        &mut slots,
        2,
        3,
        &mut presented_scene,
        topology_oracle_snapshot(
            &[
                a.clone(),
                b_content.clone(),
                moved_child.clone(),
                sibling.clone(),
            ],
            &[],
        ),
        &visuals,
        WIDTH,
        HEIGHT,
        "subsurface sibling map",
    );
    topology_oracle_present(
        &mut planner,
        &mut slots,
        0,
        3,
        &mut presented_scene,
        topology_oracle_snapshot(
            &[
                a.clone(),
                b_content.clone(),
                sibling.clone(),
                moved_child.clone(),
            ],
            &[],
        ),
        &visuals,
        WIDTH,
        HEIGHT,
        "subsurface reorder",
    );

    let rejected = topology_oracle_snapshot(&[a.clone(), b_content.clone(), moved_child], &[]);
    let presented_before_rejection = presented_scene.clone();
    topology_oracle_reject_candidate(
        &mut planner,
        &mut slots,
        1,
        3,
        &presented_scene,
        &rejected,
        &visuals,
        WIDTH,
        HEIGHT,
        "rejected candidate",
    );
    assert_eq!(presented_scene, presented_before_rejection);
    topology_oracle_present(
        &mut planner,
        &mut slots,
        2,
        3,
        &mut presented_scene,
        rejected,
        &visuals,
        WIDTH,
        HEIGHT,
        "rejected candidate retry",
    );

    topology_oracle_present(
        &mut planner,
        &mut slots,
        2,
        3,
        &mut presented_scene,
        topology_oracle_snapshot(&[a.clone(), b_content.clone()], &[]),
        &visuals,
        WIDTH,
        HEIGHT,
        "subsurface unmap",
    );

    let mut buffer_b = b_content.clone();
    buffer_b.buffer = CommittedSurfaceBuffer::shm_snapshot(
        client_buffer_b.clone(),
        buffer_b.buffer_size(),
        vec![0; buffer_b.buffer_size().width as usize * buffer_b.buffer_size().height as usize],
    );
    buffer_b.generation = 3;
    buffer_b.commit_sequence = SurfaceCommitSequence(3);
    buffer_b.damage = RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
        x: 4,
        y: 4,
        width: 12,
        height: 10,
    }]);
    topology_oracle_present(
        &mut planner,
        &mut slots,
        0,
        1,
        &mut presented_scene,
        topology_oracle_snapshot(&[a.clone(), buffer_b.clone()], &[]),
        &visuals,
        WIDTH,
        HEIGHT,
        "client buffer A to B",
    );

    let mut buffer_c = buffer_b.clone();
    buffer_c.buffer = CommittedSurfaceBuffer::shm_snapshot(
        client_buffer_c.clone(),
        buffer_c.buffer_size(),
        vec![0; buffer_c.buffer_size().width as usize * buffer_c.buffer_size().height as usize],
    );
    buffer_c.generation = 4;
    buffer_c.commit_sequence = SurfaceCommitSequence(4);
    buffer_c.damage = RenderableSurfaceDamage::Empty;
    topology_oracle_present(
        &mut planner,
        &mut slots,
        1,
        2,
        &mut presented_scene,
        topology_oracle_snapshot(&[a.clone(), buffer_c.clone()], &[]),
        &visuals,
        WIDTH,
        HEIGHT,
        "client buffer B to C authoritative Empty",
    );

    let mut buffer_a_again = buffer_c.clone();
    buffer_a_again.buffer = CommittedSurfaceBuffer::shm_snapshot(
        client_buffer_a,
        buffer_a_again.buffer_size(),
        vec![
            0;
            buffer_a_again.buffer_size().width as usize
                * buffer_a_again.buffer_size().height as usize
        ],
    );
    buffer_a_again.generation = 5;
    buffer_a_again.commit_sequence = SurfaceCommitSequence(5);
    buffer_a_again.damage = RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
        x: 18,
        y: 16,
        width: 16,
        height: 12,
    }]);
    topology_oracle_present(
        &mut planner,
        &mut slots,
        2,
        3,
        &mut presented_scene,
        topology_oracle_snapshot(&[a, buffer_a_again], &[]),
        &visuals,
        WIDTH,
        HEIGHT,
        "client buffer C to A",
    );
}
