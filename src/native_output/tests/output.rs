use super::*;

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
    assert!(message.contains("backend=native-egl-gbm"));
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
        width: 340,
        height: 230,
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
        visual_clip: Some(oblivion_one::compositor::SurfaceTargetRect::new(
            0, 0, 300, 200,
        )),
        ..test_renderable_surface(7, 0, 0, 300, 200, RenderableSurfaceDamage::Full)
    };
    let current = RenderableSurface {
        visual_clip: Some(oblivion_one::compositor::SurfaceTargetRect::new(
            0, 0, 220, 160,
        )),
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
        render_placement: None,
        visual_clip: None,
        commit_sequence: SurfaceCommitSequence::initial(),
        generation: 0,
        buffer: CommittedSurfaceBuffer::shm_snapshot(
            test_buffer_identity(),
            size,
            vec![0; width as usize * height as usize],
        ),
        viewport_source: None,
        damage,
    }
}
