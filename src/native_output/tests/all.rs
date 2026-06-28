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
            NativeScanoutKind::NativeEglGbm,
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
            NativeScanoutKind::NativeEglGbm,
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
        NativeScanoutKind::NativeEglGbm,
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
            origin.0, origin.1, 300, 200,
        )),
        ..test_renderable_surface(7, 0, 0, 300, 200, RenderableSurfaceDamage::Full)
    };
    let current = RenderableSurface {
        visual_clip: Some(oblivion_one::compositor::SurfaceTargetRect::new(
            origin.0, origin.1, 220, 160,
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

fn test_renderable_surface(
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
        damage,
    }
}

#[test]
fn proc_stat_cpu_parser_reads_user_and_system_ticks_after_comm() {
    let stat = "1234 (oblivion one) S 1 2 3 4 5 6 7 8 9 10 123 45 0 0 20 0";

    assert_eq!(
        parse_proc_stat_cpu_ticks(stat),
        Some(NativeProcessCpuSample {
            user_ticks: 123,
            system_ticks: 45,
        })
    );
}

#[test]
fn kms_mode_preference_parses_exact_resolution_and_refresh() {
    assert_eq!(
        NativeModePreference::parse("1920x1080@165"),
        NativeModePreference::Exact {
            width: 1920,
            height: 1080,
            refresh_hz: Some(165),
        }
    );
}

#[test]
fn kms_mode_selection_prefers_exact_refresh_when_available() {
    let modes = [
        test_drm_mode(1920, 1080, 60),
        test_drm_mode(2560, 1440, 144),
        test_drm_mode(1920, 1080, 165),
    ];

    let selected = select_kms_mode(
        &modes,
        NativeModePreference::Exact {
            width: 1920,
            height: 1080,
            refresh_hz: Some(165),
        },
    )
    .expect("exact mode should be selected");

    assert_eq!(mode_tuple(&selected), (1920, 1080, 165));
}

#[test]
fn kms_mode_selection_uses_nearest_refresh_for_exact_resolution() {
    let modes = [
        test_drm_mode(1920, 1080, 60),
        test_drm_mode(1920, 1080, 144),
        test_drm_mode(2560, 1440, 75),
    ];

    let selected = select_kms_mode(
        &modes,
        NativeModePreference::Exact {
            width: 1920,
            height: 1080,
            refresh_hz: Some(165),
        },
    )
    .expect("nearest refresh should be selected");

    assert_eq!(mode_tuple(&selected), (1920, 1080, 144));
}

#[test]
fn kms_mode_selection_highrr_prioritizes_refresh_then_resolution() {
    let modes = [
        test_drm_mode(3840, 2160, 60),
        test_drm_mode(2560, 1440, 144),
        test_drm_mode(1920, 1080, 165),
    ];

    let selected =
        select_kms_mode(&modes, NativeModePreference::HighRefresh).expect("mode selected");

    assert_eq!(mode_tuple(&selected), (1920, 1080, 165));
}

#[test]
fn kms_mode_selection_auto_prioritizes_resolution_then_refresh() {
    let modes = [
        test_drm_mode(1920, 1080, 165),
        test_drm_mode(2560, 1440, 60),
        test_drm_mode(2560, 1440, 144),
    ];

    let selected = select_kms_mode(&modes, NativeModePreference::Auto).expect("mode selected");

    assert_eq!(mode_tuple(&selected), (2560, 1440, 144));
}

#[test]
fn select_crtc_prefers_encoder_current_crtc_when_available() {
    let encoder = drm_sys::drm_mode_get_encoder {
        crtc_id: 42,
        possible_crtcs: 0b010,
        ..Default::default()
    };

    assert_eq!(select_crtc_id(&[12, 42, 77], &encoder), Some(42));
}

#[test]
fn select_crtc_falls_back_to_possible_crtc_bitset() {
    let encoder = drm_sys::drm_mode_get_encoder {
        crtc_id: 0,
        possible_crtcs: 0b100,
        ..Default::default()
    };

    assert_eq!(select_crtc_id(&[12, 42, 77], &encoder), Some(77));
}

fn test_drm_mode(width: u16, height: u16, refresh_hz: u32) -> drm_sys::drm_mode_modeinfo {
    drm_sys::drm_mode_modeinfo {
        hdisplay: width,
        vdisplay: height,
        vrefresh: refresh_hz,
        ..Default::default()
    }
}

fn mode_tuple(mode: &drm_sys::drm_mode_modeinfo) -> (u16, u16, u32) {
    (mode.hdisplay, mode.vdisplay, mode.vrefresh)
}

#[test]
fn native_drm_backend_plan_prefers_libseat_when_available() {
    let plan = NativeDrmBackendPlan::choose(NativeDrmBackendChoice {
        preference: NativeDrmBackendPreference::Auto,
        seat_available: true,
    });

    assert_eq!(plan.primary, NativeDrmBackendKind::Libseat);
    assert_eq!(plan.fallbacks, vec![NativeDrmBackendKind::Direct]);
}

#[test]
fn native_drm_backend_plan_uses_direct_without_seat() {
    let plan = NativeDrmBackendPlan::choose(NativeDrmBackendChoice {
        preference: NativeDrmBackendPreference::Auto,
        seat_available: false,
    });

    assert_eq!(plan.primary, NativeDrmBackendKind::Direct);
    assert!(plan.fallbacks.is_empty());
}

#[test]
fn native_drm_backend_plan_can_force_libseat() {
    let plan = NativeDrmBackendPlan::choose(NativeDrmBackendChoice {
        preference: NativeDrmBackendPreference::Libseat,
        seat_available: true,
    });

    assert_eq!(plan.primary, NativeDrmBackendKind::Libseat);
    assert!(plan.fallbacks.is_empty());
}

#[test]
fn native_drm_backend_plan_rejects_forced_libseat_without_seat() {
    let plan = NativeDrmBackendPlan::choose(NativeDrmBackendChoice {
        preference: NativeDrmBackendPreference::Libseat,
        seat_available: false,
    });

    assert_eq!(plan.primary, NativeDrmBackendKind::Unavailable);
    assert!(plan.fallbacks.is_empty());
}

#[test]
fn native_scanout_plan_prefers_native_egl_gbm_when_ready() {
    let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
        preference: NativeScanoutPreference::Auto,
        gbm_available: true,
        egl_available: true,
        page_flip_available: true,
    });

    assert_eq!(plan.primary, NativeScanoutKind::NativeEglGbm);
    assert_eq!(
        plan.fallbacks,
        vec![
            NativeScanoutKind::GbmCpuWritePageFlip,
            NativeScanoutKind::DumbFramebuffer
        ]
    );
}

#[test]
fn native_scanout_plan_can_force_gpu_without_cpu_fallback() {
    let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
        preference: NativeScanoutPreference::NativeEglGbm,
        gbm_available: true,
        egl_available: true,
        page_flip_available: true,
    });

    assert_eq!(plan.primary, NativeScanoutKind::NativeEglGbm);
    assert!(plan.fallbacks.is_empty());
}

#[test]
fn native_scanout_plan_rejects_forced_gpu_without_egl() {
    let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
        preference: NativeScanoutPreference::NativeEglGbm,
        gbm_available: true,
        egl_available: false,
        page_flip_available: true,
    });

    assert_eq!(plan.primary, NativeScanoutKind::Unavailable);
    assert!(plan.fallbacks.is_empty());
}

#[test]
fn native_scanout_plan_fallback_after_gpu_failure_preserves_remaining_candidates() {
    let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
        preference: NativeScanoutPreference::Auto,
        gbm_available: true,
        egl_available: true,
        page_flip_available: true,
    })
    .after_failed(NativeScanoutKind::NativeEglGbm);

    assert_eq!(plan.primary, NativeScanoutKind::GbmCpuWritePageFlip);
    assert_eq!(plan.fallbacks, vec![NativeScanoutKind::DumbFramebuffer]);
}

#[test]
fn native_scanout_kind_names_cpu_write_gbm_backend_honestly() {
    assert_eq!(
        NativeScanoutKind::GbmCpuWritePageFlip.as_str(),
        "GBM CPU-write pageflip"
    );
}

#[test]
fn injected_native_egl_gbm_open_failure_returns_clear_error_before_kms_use() {
    let previous = std::env::var_os("OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM");
    unsafe {
        std::env::set_var("OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM", "1");
    }
    let file = fs::File::open("Cargo.toml").unwrap();

    let error =
        match NativeScanoutBackend::open_kind(NativeScanoutKind::NativeEglGbm, &file, 1, 1, 1) {
            Ok(_) => panic!("injected native EGL/GBM failure should fail before KMS use"),
            Err(error) => error,
        };

    match previous {
        Some(value) => unsafe {
            std::env::set_var("OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM", value);
        },
        None => unsafe {
            std::env::remove_var("OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM");
        },
    }
    assert!(
        error
            .to_string()
            .contains("OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM")
    );
}

#[test]
fn auto_gpu_open_failure_next_cpu_candidate_resolves_apps_to_cpu() {
    let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
        preference: NativeScanoutPreference::Auto,
        gbm_available: true,
        egl_available: true,
        page_flip_available: true,
    });

    let fallback = plan.after_failed(NativeScanoutKind::NativeEglGbm);

    assert_eq!(fallback.primary, NativeScanoutKind::GbmCpuWritePageFlip);
    assert_eq!(
        resolve_native_app_gpu_policy(CompositorAppGpuPreference::Auto, fallback.primary).unwrap(),
        EffectiveCompositorAppGpuPolicy::CpuOnly
    );
}

#[test]
fn native_scanout_preference_keeps_legacy_gbm_egl_alias_for_cpu_write_backend() {
    assert_eq!(
        NativeScanoutPreference::parse("gbm-egl"),
        NativeScanoutPreference::GbmCpuWritePageFlip
    );
}

#[test]
fn native_scanout_preference_accepts_canonical_cpu_write_backend_name() {
    assert_eq!(
        NativeScanoutPreference::parse("gbm-cpu-write"),
        NativeScanoutPreference::GbmCpuWritePageFlip
    );
}

#[test]
fn native_scanout_preference_accepts_canonical_gpu_backend_name() {
    assert_eq!(
        NativeScanoutPreference::parse("native-egl-gbm"),
        NativeScanoutPreference::NativeEglGbm
    );
    assert_eq!(
        NativeScanoutPreference::parse("gpu"),
        NativeScanoutPreference::NativeEglGbm
    );
}

#[test]
fn native_scanout_plan_uses_cpu_gbm_fallback_without_egl() {
    let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
        preference: NativeScanoutPreference::Auto,
        gbm_available: true,
        egl_available: false,
        page_flip_available: true,
    });

    assert_eq!(plan.primary, NativeScanoutKind::GbmCpuWritePageFlip);
    assert_eq!(plan.fallbacks, vec![NativeScanoutKind::DumbFramebuffer]);
}

#[test]
fn native_pageflip_state_blocks_overlapping_flips() {
    let mut state = AtomicCommitState::default();
    let token = PageFlipToken::new(allocate_native_page_flip_token()).unwrap();
    let framebuffer = FramebufferId::new(17).unwrap();

    assert!(!state.is_pending());
    state.begin(token, framebuffer, 3, Instant::now()).unwrap();
    assert!(state.is_pending());
    assert!(
        state
            .begin(
                PageFlipToken::new(allocate_native_page_flip_token()).unwrap(),
                FramebufferId::new(18).unwrap(),
                3,
                Instant::now(),
            )
            .is_err()
    );
    assert_eq!(
        state.complete(token, 3),
        AtomicCompletion::Completed { framebuffer }
    );
    assert!(!state.is_pending());
    assert_eq!(state.complete(token, 3), AtomicCompletion::Stale);
}

#[test]
fn native_pageflip_state_rejects_mismatch_without_clearing_pending() {
    let mut state = AtomicCommitState::default();
    let expected = PageFlipToken::new(allocate_native_page_flip_token()).unwrap();
    let received = PageFlipToken::new(next_nonzero_page_flip_token(expected.get())).unwrap();
    state
        .begin(expected, FramebufferId::new(21).unwrap(), 5, Instant::now())
        .unwrap();

    assert_eq!(state.complete(received, 5), AtomicCompletion::Mismatched);
    assert_eq!(state.pending_token(), Some(expected));
}

#[test]
fn native_pageflip_state_stale_event_cannot_complete_new_submission() {
    let mut state = AtomicCommitState::default();
    let first = PageFlipToken::new(allocate_native_page_flip_token()).unwrap();
    let first_framebuffer = FramebufferId::new(31).unwrap();
    state
        .begin(first, first_framebuffer, 7, Instant::now())
        .unwrap();
    assert_eq!(
        state.complete(first, 7),
        AtomicCompletion::Completed {
            framebuffer: first_framebuffer
        }
    );
    let second = PageFlipToken::new(allocate_native_page_flip_token()).unwrap();
    state
        .begin(second, FramebufferId::new(32).unwrap(), 7, Instant::now())
        .unwrap();

    assert_eq!(state.complete(first, 7), AtomicCompletion::Mismatched);
    assert_eq!(state.pending_token(), Some(second));
}

#[test]
fn native_pageflip_token_wrap_skips_zero() {
    assert_eq!(next_nonzero_page_flip_token(u64::MAX), 1);
    assert_eq!(next_nonzero_page_flip_token(1), 2);
}

#[test]
fn native_pageflip_token_does_not_restart_after_backend_recreation() {
    let mut first = AtomicCommitState::default();
    let old_token = PageFlipToken::new(allocate_native_page_flip_token()).unwrap();
    let framebuffer = FramebufferId::new(41).unwrap();
    first
        .begin(old_token, framebuffer, 11, Instant::now())
        .unwrap();
    assert_eq!(
        first.complete(old_token, 11),
        AtomicCompletion::Completed { framebuffer }
    );
    let mut replacement = AtomicCommitState::default();
    let replacement_token = PageFlipToken::new(allocate_native_page_flip_token()).unwrap();
    replacement
        .begin(
            replacement_token,
            FramebufferId::new(42).unwrap(),
            12,
            Instant::now(),
        )
        .unwrap();

    assert_ne!(replacement_token, old_token);
    assert_eq!(
        replacement.complete(old_token, 11),
        AtomicCompletion::StaleGeneration
    );
    assert_eq!(replacement.pending_token(), Some(replacement_token));
}

#[test]
fn native_pageflip_buffers_promote_ready_to_pending_to_current() {
    let mut buffers = NativePageFlipBuffers::default();

    buffers.set_ready(10);
    assert_eq!(buffers.ready_or_current(), Some(&10));
    assert_eq!(buffers.take_ready(), Some(10));
    buffers.set_pending(10);
    assert!(buffers.complete_page_flip());
    assert_eq!(buffers.ready_or_current(), Some(&10));

    assert!(!buffers.complete_page_flip());
}

#[test]
fn native_pageflip_buffers_finish_initial_scanout_promotes_ready() {
    let mut buffers = NativePageFlipBuffers::default();

    buffers.set_ready(20);
    buffers.finish_initial_scanout();

    assert_eq!(buffers.ready_or_current(), Some(&20));
    assert_eq!(buffers.take_ready(), None);
}

#[test]
fn native_initial_frame_paints_real_topbar() {
    let mut renderer = NativeFrameRenderer::default();
    let spotlight = SpotlightModel::default();
    let frame = renderer
        .render_frame(NativeFrameRequest {
            width: 320,
            height: 200,
            surfaces: &[],
            dock_items: Vec::new(),
            spotlight: &spotlight,
            shell_generation: 0,
            visual_state: DesktopVisualState::wallpaper_only(),
            render_generation: 0,
            client_cursor: None,
        })
        .pixels
        .to_vec();
    let mut wallpaper = vec![0; 320 * 200];
    compose_nested_output(
        &mut wallpaper,
        320,
        200,
        &[],
        DesktopVisualState::wallpaper_only(),
    );

    assert_ne!(frame[16 * 320 + 160], wallpaper[16 * 320 + 160]);
}

#[test]
fn native_initial_frame_does_not_draw_closed_spotlight_panel() {
    let mut renderer = NativeFrameRenderer::default();
    let spotlight = SpotlightModel::default();
    let frame = renderer
        .render_frame(NativeFrameRequest {
            width: 320,
            height: 200,
            surfaces: &[],
            dock_items: Vec::new(),
            spotlight: &spotlight,
            shell_generation: 0,
            visual_state: DesktopVisualState::wallpaper_only(),
            render_generation: 0,
            client_cursor: None,
        })
        .pixels
        .to_vec();

    let mut wallpaper = vec![0; 320 * 200];
    compose_nested_output(
        &mut wallpaper,
        320,
        200,
        &[],
        DesktopVisualState::wallpaper_only(),
    );

    assert_eq!(frame[100 * 320 + 160], wallpaper[100 * 320 + 160]);
}

#[test]
fn native_input_ctrl_space_opens_spotlight_without_forwarding_space() {
    let mut input = NativeInputState::new(320, 200);

    let ctrl = input.handle_key_event(KEY_LEFTCTRL, 1);
    let space = input.handle_key_event(KEY_SPACE, 1);

    assert_eq!(
        ctrl.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_LEFTCTRL, true)]
    );
    assert!(space.redraw_requested);
    assert_eq!(
        space.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_LEFTCTRL, false)]
    );
    assert!(space.requires_frame_repaint(NativeCursorRenderMode::Hardware));
    assert!(input.spotlight_visible());
}

#[test]
fn native_input_visible_spotlight_collects_typed_letters() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTCTRL, 1);
    input.handle_key_event(KEY_SPACE, 1);

    let effect = input.handle_key_event(KEY_Z, 1);

    assert!(effect.redraw_requested);
    assert!(effect.keyboard_events.is_empty());
    assert_eq!(input.spotlight_query(), "z");
}

#[test]
fn spotlight_motion_updates_visual_cursor_without_forwarding_client_motion() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTCTRL, 1);
    input.handle_key_event(KEY_SPACE, 1);

    let effect = input.handle_pointer_motion_delta(24.0, 12.0);

    assert_eq!(input.cursor_position(), (184, 112));
    assert_eq!(effect.cursor_position, Some((184, 112)));
    assert_eq!(effect.pointer_motion, None);
    assert_eq!(effect.relative_motion, None);
}

#[test]
fn spotlight_locked_motion_does_not_leak_relative_or_absolute_motion() {
    let mut input = NativeInputState::new(320, 200);
    let anchor = input.cursor_position_f64();
    input.set_pointer_locked_at(anchor);
    input.handle_key_event(KEY_LEFTCTRL, 1);
    input.handle_key_event(KEY_SPACE, 1);

    let effect = input.handle_pointer_motion_delta(24.0, 12.0);

    assert_eq!(input.cursor_position_f64(), anchor);
    assert_eq!(effect.cursor_position, None);
    assert_eq!(effect.pointer_motion, None);
    assert_eq!(effect.relative_motion, None);
}

#[test]
fn spotlight_client_cursor_repaints_even_with_hardware_cursor_mode() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTCTRL, 1);
    input.handle_key_event(KEY_SPACE, 1);
    let effect = input.handle_pointer_motion_delta(24.0, 12.0);
    let mut updated_position = None;

    let compositor_visual_changed = apply_compositor_only_pointer_position(&effect, |x, y| {
        updated_position = Some((x, y));
        true
    });
    let redraw_requested = effect.requires_frame_repaint(NativeCursorRenderMode::Hardware)
        || compositor_visual_changed;

    assert_eq!(updated_position, Some((184.0, 112.0)));
    assert!(redraw_requested);
}

#[test]
fn ordinary_motion_does_not_apply_compositor_only_position_update() {
    let mut input = NativeInputState::new(320, 200);
    let effect = input.handle_pointer_motion_delta(24.0, 12.0);

    let compositor_visual_changed = apply_compositor_only_pointer_position(&effect, |_, _| {
        panic!("ordinary forwarded motion must not update position twice")
    });

    assert_eq!(effect.pointer_motion, Some((184.0, 112.0)));
    assert!(!compositor_visual_changed);
}

#[test]
fn native_input_alt_p_requests_session_exit_without_forwarding_p() {
    let mut input = NativeInputState::new(320, 200);

    input.handle_key_event(KEY_LEFTALT, 1);
    let p = input.handle_key_event(KEY_P, 1);

    assert!(p.exit_requested);
    assert!(p.keyboard_events.is_empty());
}

#[test]
fn native_input_ctrl_c_is_forwarded_to_clients() {
    let mut input = NativeInputState::new(320, 200);

    let ctrl = input.handle_key_event(KEY_LEFTCTRL, 1);
    let c = input.handle_key_event(KEY_C, 1);

    assert_eq!(
        ctrl.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_LEFTCTRL, true)]
    );
    assert!(!c.exit_requested);
    assert_eq!(
        c.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_C, true)]
    );
}

#[test]
fn native_input_alt_mouse_buttons_start_window_interactions() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTALT, 1);

    let move_start = input.handle_pointer_button(u32::from(BTN_LEFT), true);
    let motion = input.handle_pointer_motion_delta(24.0, 12.0);
    let move_end = input.handle_pointer_button(u32::from(BTN_LEFT), false);

    assert_eq!(
        move_start.window_actions,
        vec![NativeWindowAction::BeginMove { x: 160.0, y: 100.0 }]
    );
    assert!(move_start.pointer_buttons.is_empty());
    assert_eq!(
        motion.window_actions,
        vec![NativeWindowAction::UpdateInteraction { x: 184.0, y: 112.0 }]
    );
    assert!(motion.pointer_motion.is_none());
    assert_eq!(
        move_end.window_actions,
        vec![NativeWindowAction::EndInteraction]
    );
}

#[test]
fn native_input_unlocked_relative_motion_moves_cursor_and_preserves_relative_delta() {
    let mut input = NativeInputState::new(320, 200);

    let effect = input.handle_pointer_motion_delta(24.0, 12.0);

    assert_eq!(input.cursor_position(), (184, 112));
    assert_eq!(
        effect.relative_motion,
        Some(RelativeMotion::accelerated_only(24.0, 12.0))
    );
    assert_eq!(effect.pointer_motion, Some((184.0, 112.0)));
    assert_eq!(effect.cursor_position, Some((184, 112)));
}

#[test]
fn native_input_locked_relative_motion_preserves_delta_without_moving_cursor() {
    let mut input = NativeInputState::new(320, 200);
    input.set_pointer_locked_at(input.cursor_position_f64());

    let effect = input.handle_pointer_motion_delta(24.0, 12.0);

    assert_eq!(input.cursor_position(), (160, 100));
    assert_eq!(
        effect.relative_motion,
        Some(RelativeMotion::accelerated_only(24.0, 12.0))
    );
    assert_eq!(effect.pointer_motion, None);
    assert_eq!(effect.cursor_position, None);
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Software));
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
}

#[test]
fn native_input_locked_absolute_motion_does_not_move_cursor() {
    let mut input = NativeInputState::new(320, 200);
    input.set_pointer_locked_at(input.cursor_position_f64());

    let effect = input.handle_pointer_motion(PointerMotionSample::absolute(7, 25.0, 26.0));

    assert_eq!(input.cursor_position(), (160, 100));
    assert_eq!(effect.pointer_motion, None);
    assert_eq!(effect.cursor_position, None);
}

#[test]
fn locked_pointer_motion_never_accumulates_absolute_cursor_position() {
    let mut input = NativeInputState::new(800, 600);
    let anchor = CompositorOutputPosition {
        x: 400.25,
        y: 300.75,
    };
    input.restore_cursor_position(anchor);
    input.set_pointer_locked_at(anchor);

    let mut total_dx = 0.0;
    let mut total_dy = 0.0;
    for _ in 0..100 {
        let effect = input.handle_pointer_motion(PointerMotionSample::relative(
            10,
            RelativeMotion::accelerated_only(20.0, -15.0),
        ));
        total_dx += effect.relative_motion.unwrap().dx;
        total_dy += effect.relative_motion.unwrap().dy;
        assert_eq!(effect.pointer_motion, None);
        assert_eq!(effect.cursor_position, None);
    }

    assert_eq!(total_dx, 2000.0);
    assert_eq!(total_dy, -1500.0);
    assert_eq!(input.cursor_position_f64(), anchor);
}

#[test]
fn native_input_unlock_restore_sets_logical_cursor_position() {
    let mut input = NativeInputState::new(320, 200);
    input.set_pointer_locked_at(input.cursor_position_f64());
    input.handle_pointer_motion_delta(200.0, 200.0);

    input.clear_pointer_constraint();
    let effect = input.restore_cursor_position(CompositorOutputPosition { x: 35.0, y: 45.0 });

    assert_eq!(input.cursor_position(), (35, 45));
    assert_eq!(effect.cursor_position, Some((35, 45)));
    assert!(effect.requires_frame_repaint(NativeCursorRenderMode::Software));
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
}

#[test]
fn native_input_first_real_move_after_unlock_starts_from_restored_position() {
    let mut input = NativeInputState::new(320, 200);
    input.set_pointer_locked_at(input.cursor_position_f64());
    input.handle_pointer_motion_delta(200.0, 200.0);
    input.clear_pointer_constraint();
    input.restore_cursor_position(CompositorOutputPosition { x: 35.0, y: 45.0 });

    let effect = input.handle_pointer_motion_delta(5.0, -10.0);

    assert_eq!(input.cursor_position(), (40, 35));
    assert_eq!(effect.pointer_motion, Some((40.0, 35.0)));
}

#[test]
fn native_input_confined_relative_motion_clamps_to_region_and_keeps_absolute_motion() {
    let mut input = NativeInputState::new(320, 200);
    input.restore_cursor_position(CompositorOutputPosition { x: 50.0, y: 50.0 });
    input.set_pointer_confined(OutputRegion::from_rect(
        OutputRect::new(40.0, 30.0, 60.0, 40.0).unwrap(),
    ));

    let right = input.handle_pointer_motion_delta(100.0, 0.0);
    assert_eq!(input.cursor_position(), (99, 50));
    assert_eq!(right.pointer_motion, Some((99.0, 50.0)));
    assert_eq!(
        right.relative_motion,
        Some(RelativeMotion::accelerated_only(100.0, 0.0))
    );

    let bottom = input.handle_pointer_motion_delta(0.0, 100.0);
    assert_eq!(input.cursor_position(), (99, 69));
    assert_eq!(bottom.pointer_motion, Some((99.0, 69.0)));

    let left = input.handle_pointer_motion_delta(-100.0, 0.0);
    assert_eq!(input.cursor_position(), (40, 69));
    assert_eq!(left.pointer_motion, Some((40.0, 69.0)));

    let top = input.handle_pointer_motion_delta(0.0, -100.0);
    assert_eq!(input.cursor_position(), (40, 30));
    assert_eq!(top.pointer_motion, Some((40.0, 30.0)));
}

#[test]
fn native_input_confined_absolute_motion_clamps_and_requests_cursor_repaint() {
    let mut input = NativeInputState::new(320, 200);
    input.set_pointer_confined(OutputRegion::from_rect(
        OutputRect::new(40.0, 30.0, 60.0, 40.0).unwrap(),
    ));

    let effect = input.handle_pointer_motion(PointerMotionSample::absolute(7, 500.0, 1.0));

    assert_eq!(input.cursor_position(), (99, 30));
    assert_eq!(effect.pointer_motion, Some((99.0, 30.0)));
    assert_eq!(effect.cursor_position, Some((99, 30)));
    assert!(effect.requires_frame_repaint(NativeCursorRenderMode::Software));
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
}

#[test]
fn native_pointer_constraint_backend_activates_locked_once() {
    let mut backend = NativePointerConstraintBackend::new();
    let id = PointerConstraintBackendId {
        constraint_id: 7,
        generation: 1,
    };

    let first = backend.handle_request(
        PointerConstraintBackendRequest::ActivateLocked {
            id,
            anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
        },
        CompositorOutputPosition { x: 10.0, y: 20.0 },
    );
    let duplicate = backend.handle_request(
        PointerConstraintBackendRequest::ActivateLocked {
            id,
            anchor: CompositorOutputPosition { x: 30.0, y: 40.0 },
        },
        CompositorOutputPosition { x: 30.0, y: 40.0 },
    );

    assert_eq!(
        first.activated,
        Some(NativePointerConstraint {
            id,
            mode: PointerConstraintMode::Locked,
            anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
            region: None,
        })
    );
    assert_eq!(duplicate, NativePointerConstraintBackendAction::default());
    assert!(backend.active_locked());
}

#[test]
fn native_pointer_constraint_backend_activates_confined_with_region() {
    let mut backend = NativePointerConstraintBackend::new();
    let id = PointerConstraintBackendId {
        constraint_id: 8,
        generation: 1,
    };
    let region = OutputRegion::from_rect(OutputRect::new(10.0, 20.0, 100.0, 50.0).unwrap());

    let action = backend.handle_request(
        PointerConstraintBackendRequest::ActivateConfined {
            id,
            region: region.clone(),
        },
        CompositorOutputPosition { x: 10.0, y: 20.0 },
    );

    assert_eq!(
        action.activated,
        Some(NativePointerConstraint {
            id,
            mode: PointerConstraintMode::Confined,
            anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
            region: Some(region),
        })
    );
    assert!(action.failed.is_none());
}

#[test]
fn native_pointer_constraint_backend_mismatched_deactivation_cannot_unlock_newer_lock() {
    let mut backend = NativePointerConstraintBackend::new();
    let active = PointerConstraintBackendId {
        constraint_id: 9,
        generation: 2,
    };
    let stale = PointerConstraintBackendId {
        constraint_id: 9,
        generation: 1,
    };
    backend.handle_request(
        PointerConstraintBackendRequest::ActivateLocked {
            id: active,
            anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
        },
        CompositorOutputPosition { x: 10.0, y: 20.0 },
    );

    let action = backend.handle_request(
        PointerConstraintBackendRequest::Deactivate {
            id: stale,
            restore_position: Some(CompositorOutputPosition { x: 40.0, y: 50.0 }),
        },
        CompositorOutputPosition { x: 99.0, y: 99.0 },
    );

    assert_eq!(action, NativePointerConstraintBackendAction::default());
    assert!(backend.active_locked());
}

#[test]
fn native_pointer_constraint_backend_deactivation_restores_hint_or_anchor() {
    let mut backend = NativePointerConstraintBackend::new();
    let id = PointerConstraintBackendId {
        constraint_id: 10,
        generation: 1,
    };
    backend.handle_request(
        PointerConstraintBackendRequest::ActivateLocked {
            id,
            anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
        },
        CompositorOutputPosition { x: 10.0, y: 20.0 },
    );

    let action = backend.handle_request(
        PointerConstraintBackendRequest::Deactivate {
            id,
            restore_position: Some(CompositorOutputPosition { x: 30.0, y: 40.0 }),
        },
        CompositorOutputPosition { x: 99.0, y: 99.0 },
    );

    assert_eq!(action.deactivated, Some(id));
    assert_eq!(
        action.restore_position,
        Some(CompositorOutputPosition { x: 30.0, y: 40.0 })
    );
    assert!(!backend.active_locked());

    backend.handle_request(
        PointerConstraintBackendRequest::ActivateLocked {
            id,
            anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
        },
        CompositorOutputPosition { x: 10.0, y: 20.0 },
    );
    let action = backend.handle_request(
        PointerConstraintBackendRequest::Deactivate {
            id,
            restore_position: None,
        },
        CompositorOutputPosition { x: 99.0, y: 99.0 },
    );

    assert_eq!(
        action.restore_position,
        Some(CompositorOutputPosition { x: 10.0, y: 20.0 })
    );
}

#[test]
fn native_pointer_constraint_backend_preserves_fractional_activation_anchor() {
    let mut backend = NativePointerConstraintBackend::new();
    let id = PointerConstraintBackendId {
        constraint_id: 12,
        generation: 3,
    };
    let anchor = CompositorOutputPosition {
        x: 400.25,
        y: 300.75,
    };

    backend.handle_request(
        PointerConstraintBackendRequest::ActivateLocked { id, anchor },
        anchor,
    );
    let action = backend.handle_request(
        PointerConstraintBackendRequest::Deactivate {
            id,
            restore_position: None,
        },
        CompositorOutputPosition { x: 0.0, y: 0.0 },
    );

    assert_eq!(action.restore_position, Some(anchor));
}

#[test]
fn native_pointer_constraint_backend_confined_deactivation_does_not_restore_anchor() {
    let mut backend = NativePointerConstraintBackend::new();
    let id = PointerConstraintBackendId {
        constraint_id: 11,
        generation: 1,
    };
    backend.handle_request(
        PointerConstraintBackendRequest::ActivateConfined {
            id,
            region: OutputRegion::from_rect(OutputRect::new(10.0, 20.0, 100.0, 50.0).unwrap()),
        },
        CompositorOutputPosition { x: 30.0, y: 40.0 },
    );

    let action = backend.handle_request(
        PointerConstraintBackendRequest::Deactivate {
            id,
            restore_position: None,
        },
        CompositorOutputPosition { x: 99.0, y: 99.0 },
    );

    assert_eq!(action.deactivated, Some(id));
    assert_eq!(action.restore_position, None);
    assert!(!backend.active_locked());
}

#[test]
fn native_pointer_constraint_backend_updates_confined_region_in_place() {
    let mut backend = NativePointerConstraintBackend::new();
    let id = PointerConstraintBackendId {
        constraint_id: 12,
        generation: 1,
    };
    backend.handle_request(
        PointerConstraintBackendRequest::ActivateConfined {
            id,
            region: OutputRegion::from_rect(OutputRect::new(10.0, 20.0, 100.0, 50.0).unwrap()),
        },
        CompositorOutputPosition { x: 30.0, y: 40.0 },
    );

    let action = backend.handle_request(
        PointerConstraintBackendRequest::UpdateConfinedRegion {
            id,
            region: OutputRegion::from_rect(OutputRect::new(40.0, 50.0, 20.0, 10.0).unwrap()),
        },
        CompositorOutputPosition { x: 30.0, y: 40.0 },
    );

    assert_eq!(action.deactivated, None);
    assert_eq!(action.activated, None);
    assert_eq!(
        action.cursor_position,
        Some(CompositorOutputPosition { x: 40.0, y: 50.0 })
    );
    assert_eq!(action.restore_position, None);
    assert_eq!(
        backend.active_constraint_state(),
        NativePointerConstraintState::Confined {
            region: OutputRegion::from_rect(OutputRect::new(40.0, 50.0, 20.0, 10.0).unwrap())
        }
    );
}

#[test]
fn native_pointer_constraint_backend_tracks_cursor_visibility_changes() {
    let mut backend = NativePointerConstraintBackend::new();

    let hide = backend.handle_request(
        PointerConstraintBackendRequest::ApplyCursorVisibility { visible: false },
        CompositorOutputPosition::default(),
    );
    let duplicate_hide = backend.handle_request(
        PointerConstraintBackendRequest::ApplyCursorVisibility { visible: false },
        CompositorOutputPosition::default(),
    );
    let show = backend.handle_request(
        PointerConstraintBackendRequest::ApplyCursorVisibility { visible: true },
        CompositorOutputPosition::default(),
    );

    assert_eq!(hide.cursor_visibility_changed, Some(false));
    assert_eq!(
        duplicate_hide,
        NativePointerConstraintBackendAction::default()
    );
    assert_eq!(show.cursor_visibility_changed, Some(true));
}

#[test]
fn native_input_alt_right_starts_window_resize() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_RIGHTALT, 1);

    let effect = input.handle_pointer_button(u32::from(BTN_RIGHT), true);

    assert_eq!(
        effect.window_actions,
        vec![NativeWindowAction::BeginResize { x: 160.0, y: 100.0 }]
    );
    assert!(effect.pointer_buttons.is_empty());
}

#[test]
fn native_input_alt_release_ends_active_window_interaction() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTALT, 1);
    input.handle_pointer_button(u32::from(BTN_LEFT), true);

    let effect = input.handle_key_event(KEY_LEFTALT, 0);

    assert_eq!(
        effect.window_actions,
        vec![NativeWindowAction::EndInteraction]
    );
}

#[test]
fn native_input_alt_keyboard_shortcuts_map_to_window_actions() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTALT, 1);

    let minimize = input.handle_key_event(KEY_M, 1);
    let minimize_release = input.handle_key_event(KEY_M, 0);
    let restore = input.handle_key_event(KEY_R, 1);
    let maximize = input.handle_key_event(KEY_F, 1);
    let fullscreen = input.handle_key_event(KEY_F11, 1);

    assert_eq!(minimize.window_actions, vec![NativeWindowAction::Minimize]);
    assert!(minimize.keyboard_events.is_empty());
    assert!(minimize_release.keyboard_events.is_empty());
    assert_eq!(
        restore.window_actions,
        vec![NativeWindowAction::RestoreMinimized]
    );
    assert_eq!(
        maximize.window_actions,
        vec![NativeWindowAction::ToggleMaximize]
    );
    assert_eq!(
        fullscreen.window_actions,
        vec![NativeWindowAction::ToggleFullscreen]
    );
}

#[test]
fn native_input_shortcut_inhibition_forwards_window_shortcuts_to_client() {
    let mut input = NativeInputState::new(320, 200);
    input.set_keyboard_shortcuts_inhibited(true);

    let alt = input.handle_key_event(KEY_LEFTALT, 1);
    let fullscreen = input.handle_key_event(KEY_F11, 1);

    assert_eq!(
        alt.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_LEFTALT, true)]
    );
    assert!(alt.window_actions.is_empty());
    assert_eq!(
        fullscreen.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_F11, true)]
    );
    assert!(fullscreen.window_actions.is_empty());
}

#[test]
fn native_input_shortcut_inhibition_keeps_emergency_exit_shortcut() {
    let mut input = NativeInputState::new(320, 200);
    input.set_keyboard_shortcuts_inhibited(true);
    input.handle_key_event(KEY_LEFTALT, 1);

    let effect = input.handle_key_event(KEY_P, 1);

    assert!(effect.exit_requested);
    assert!(effect.keyboard_events.is_empty());
}

#[test]
fn native_input_backend_plan_prefers_libseat_libinput_when_available() {
    let plan = NativeInputBackendPlan::choose(NativeInputBackendChoice {
        preference: NativeInputBackendPreference::Auto,
        libseat_available: true,
        libinput_available: true,
        raw_evdev_available: true,
    });

    assert_eq!(plan.primary, NativeInputBackendKind::LibseatLibinputUdev);
    assert_eq!(
        plan.fallbacks,
        vec![
            NativeInputBackendKind::DirectLibinputUdev,
            NativeInputBackendKind::RawEvdev,
        ]
    );
}

#[test]
fn native_input_backend_plan_uses_direct_libinput_without_libseat() {
    let plan = NativeInputBackendPlan::choose(NativeInputBackendChoice {
        preference: NativeInputBackendPreference::Auto,
        libseat_available: false,
        libinput_available: true,
        raw_evdev_available: true,
    });

    assert_eq!(plan.primary, NativeInputBackendKind::DirectLibinputUdev);
    assert_eq!(plan.fallbacks, vec![NativeInputBackendKind::RawEvdev]);
}

#[test]
fn native_input_backend_plan_can_force_raw_evdev_for_debugging() {
    let plan = NativeInputBackendPlan::choose(NativeInputBackendChoice {
        preference: NativeInputBackendPreference::RawEvdev,
        libseat_available: true,
        libinput_available: true,
        raw_evdev_available: true,
    });

    assert_eq!(plan.primary, NativeInputBackendKind::RawEvdev);
    assert!(plan.fallbacks.is_empty());
}

#[test]
fn native_input_backend_plan_falls_back_to_raw_when_libinput_is_unavailable() {
    let plan = NativeInputBackendPlan::choose(NativeInputBackendChoice {
        preference: NativeInputBackendPreference::Auto,
        libseat_available: true,
        libinput_available: false,
        raw_evdev_available: true,
    });

    assert_eq!(plan.primary, NativeInputBackendKind::RawEvdev);
    assert!(plan.fallbacks.is_empty());
}

#[test]

fn native_seat_lifecycle_requests_suspend_then_resume() {
    let mut lifecycle = NativeSeatLifecycle::default();

    assert_eq!(
        lifecycle.apply_event(NativeSeatEvent::Disabled),
        Some(NativeSeatInputAction::Suspend)
    );
    assert_eq!(
        lifecycle.apply_event(NativeSeatEvent::Enabled),
        Some(NativeSeatInputAction::Resume)
    );
}

#[test]
fn native_input_state_handles_normalized_relative_motion() {
    let mut input = NativeInputState::new(320, 200);

    let effect = input.handle_hardware_input_event(NativeHardwareInputEvent::PointerMotion(
        PointerMotionSample::relative(10, RelativeMotion::accelerated_only(12.0, -4.0)),
    ));

    assert_eq!(effect.pointer_motion, Some((172.0, 96.0)));
    assert!(effect.redraw_requested);
}

#[test]
fn native_input_pointer_motion_can_skip_frame_repaint_with_hardware_cursor() {
    let mut input = NativeInputState::new(320, 200);

    let effect = input.handle_hardware_input_event(NativeHardwareInputEvent::PointerMotion(
        PointerMotionSample::relative(10, RelativeMotion::accelerated_only(12.0, -4.0)),
    ));

    assert_eq!(effect.pointer_motion, Some((172.0, 96.0)));
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
    assert!(effect.requires_frame_repaint(NativeCursorRenderMode::Software));
}

#[test]
fn native_forwarded_keyboard_input_skips_frame_repaint_without_local_visual_change() {
    let mut input = NativeInputState::new(320, 200);

    let effect = input.handle_key_event(KEY_Z, 1);

    assert_eq!(
        effect.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_Z, true)]
    );
    assert!(effect.redraw_requested);
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Software));
}

#[test]
fn native_forwarded_pointer_button_skips_frame_repaint_without_local_visual_change() {
    let mut input = NativeInputState::new(320, 200);

    let effect = input.handle_pointer_button(u32::from(BTN_LEFT), true);

    assert_eq!(effect.pointer_buttons.len(), 1);
    assert!(effect.redraw_requested);
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Software));
}

#[test]
fn native_forwarded_pointer_axis_skips_frame_repaint_without_local_visual_change() {
    let mut input = NativeInputState::new(320, 200);

    let effect = input.handle_pointer_axis(0.0, 120.0);

    assert_eq!(effect.pointer_axis, Some((0.0, 120.0)));
    assert!(effect.redraw_requested);
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Software));
}

#[test]
fn native_input_window_interaction_still_repaints_with_hardware_cursor() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTALT, 1);
    input.handle_pointer_button(u32::from(BTN_LEFT), true);

    let effect = input.handle_hardware_input_event(NativeHardwareInputEvent::PointerMotion(
        PointerMotionSample::relative(10, RelativeMotion::accelerated_only(12.0, -4.0)),
    ));

    assert_eq!(
        effect.window_actions,
        vec![NativeWindowAction::UpdateInteraction { x: 172.0, y: 96.0 }]
    );
    assert!(effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
}

#[test]
fn native_libinput_scroll_axis_value_skips_absent_axis_reader() {
    let value = libinput_scroll_axis_value(false, || panic!("axis value should not be read"));

    assert_eq!(value, 0.0);
}

#[test]
fn native_xrgb_copy_preserves_ignored_high_byte_for_fast_row_copy() {
    let frame = [0x7f11_2233];
    let mut bytes = [0; 4];

    copy_argb_frame_to_xrgb_mapping(&frame, 1, 1, 4, &mut bytes).unwrap();

    assert_eq!(bytes, 0x7f11_2233u32.to_ne_bytes());
}

#[test]
fn native_xrgb_copy_damage_updates_only_requested_rectangles() {
    let frame = [0xff00_0001, 0xff00_0002, 0xff00_0003, 0xff00_0004];
    let untouched = 0xa5;
    let mut bytes = [untouched; 16];

    let copied = copy_argb_frame_to_xrgb_mapping_damage(
        &frame,
        2,
        2,
        8,
        &mut bytes,
        NativeFrameCopyDamage::Rects(&[NativeDamageRect {
            x: 1,
            y: 0,
            width: 1,
            height: 2,
        }]),
    )
    .unwrap();

    assert_eq!(copied, 8);
    assert_eq!(&bytes[0..4], &[untouched; 4]);
    assert_eq!(&bytes[4..8], &0xff00_0002u32.to_ne_bytes());
    assert_eq!(&bytes[8..12], &[untouched; 4]);
    assert_eq!(&bytes[12..16], &0xff00_0004u32.to_ne_bytes());
}

#[test]
fn native_xrgb_copy_damage_caps_overlapping_rects_at_full_frame_copy() {
    let frame = [0xff00_0001, 0xff00_0002, 0xff00_0003, 0xff00_0004];
    let mut bytes = [0; 16];

    let copied = copy_argb_frame_to_xrgb_mapping_damage(
        &frame,
        2,
        2,
        8,
        &mut bytes,
        NativeFrameCopyDamage::Rects(&[
            NativeDamageRect {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
            NativeDamageRect {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
        ]),
    )
    .unwrap();

    assert_eq!(copied, 16);
    assert_eq!(&bytes[0..4], &0xff00_0001u32.to_ne_bytes());
    assert_eq!(&bytes[4..8], &0xff00_0002u32.to_ne_bytes());
    assert_eq!(&bytes[8..12], &0xff00_0003u32.to_ne_bytes());
    assert_eq!(&bytes[12..16], &0xff00_0004u32.to_ne_bytes());
}

#[test]
fn native_frame_renderer_repairs_surface_bounds_change_with_partial_scene_rebuild() {
    let spotlight = SpotlightModel::default();
    let mut renderer = NativeFrameRenderer::default();
    let initial_surface = test_renderable_surface(7, 0, 0, 4, 4, RenderableSurfaceDamage::Full);

    let initial = renderer.render_frame(NativeFrameRequest {
        width: 96,
        height: 96,
        surfaces: &[initial_surface],
        dock_items: Vec::new(),
        spotlight: &spotlight,
        shell_generation: 1,
        visual_state: DesktopVisualState::wallpaper_only(),
        render_generation: 1,
        client_cursor: None,
    });
    assert_eq!(initial.scene_rebuild_kind, DesktopSceneRebuildKind::Full);

    let moved_surface = test_renderable_surface(
        7,
        2,
        0,
        4,
        4,
        RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        }]),
    );

    let moved = renderer.render_frame(NativeFrameRequest {
        width: 96,
        height: 96,
        surfaces: &[moved_surface],
        dock_items: Vec::new(),
        spotlight: &spotlight,
        shell_generation: 1,
        visual_state: DesktopVisualState::wallpaper_only(),
        render_generation: 2,
        client_cursor: None,
    });

    assert_eq!(moved.scene_rebuild_kind, DesktopSceneRebuildKind::Partial);
    assert_eq!(moved.frame_copy_kind, DesktopFrameCopyKind::Partial);
}

#[test]
fn native_frame_renderer_reports_full_scene_rebuild_when_surface_identity_changes() {
    let spotlight = SpotlightModel::default();
    let mut renderer = NativeFrameRenderer::default();
    let initial_surface = test_renderable_surface(7, 0, 0, 4, 4, RenderableSurfaceDamage::Full);

    let initial = renderer.render_frame(NativeFrameRequest {
        width: 96,
        height: 96,
        surfaces: &[initial_surface],
        dock_items: Vec::new(),
        spotlight: &spotlight,
        shell_generation: 1,
        visual_state: DesktopVisualState::wallpaper_only(),
        render_generation: 1,
        client_cursor: None,
    });
    assert_eq!(initial.scene_rebuild_kind, DesktopSceneRebuildKind::Full);

    let replacement_surface = test_renderable_surface(
        8,
        0,
        0,
        4,
        4,
        RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        }]),
    );

    let replacement = renderer.render_frame(NativeFrameRequest {
        width: 96,
        height: 96,
        surfaces: &[replacement_surface],
        dock_items: Vec::new(),
        spotlight: &spotlight,
        shell_generation: 1,
        visual_state: DesktopVisualState::wallpaper_only(),
        render_generation: 2,
        client_cursor: None,
    });

    assert_eq!(
        replacement.scene_rebuild_kind,
        DesktopSceneRebuildKind::Full
    );
}

#[test]
fn native_cursor_argb_bytes_places_texture_pixels_in_pitched_buffer() {
    let pixels = [0xff11_2233, 0x8044_5566, 0xff77_8899, 0];

    let bytes = native_cursor_argb_bytes(&pixels, 2, 2, 4, 4, 16).unwrap();

    assert_eq!(&bytes[0..4], &0xff11_2233u32.to_ne_bytes());
    assert_eq!(&bytes[4..8], &0x8044_5566u32.to_ne_bytes());
    assert_eq!(&bytes[16..20], &0xff77_8899u32.to_ne_bytes());
    assert_eq!(&bytes[20..24], &0u32.to_ne_bytes());
    assert!(bytes[24..].iter().all(|byte| *byte == 0));
}

#[test]
fn native_input_coalesces_consecutive_relative_motion_events() {
    let events = coalesce_pointer_motion_events(vec![
        NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
            10,
            RelativeMotion {
                dx: 1.0,
                dy: 0.0,
                dx_unaccelerated: 2.0,
                dy_unaccelerated: 0.0,
            },
        )),
        NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
            20,
            RelativeMotion {
                dx: 0.0,
                dy: 2.0,
                dx_unaccelerated: 0.0,
                dy_unaccelerated: 3.0,
            },
        )),
        NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
            30,
            RelativeMotion {
                dx: 3.0,
                dy: 4.0,
                dx_unaccelerated: 5.0,
                dy_unaccelerated: 6.0,
            },
        )),
    ]);

    assert_eq!(
        events,
        vec![NativeHardwareInputEvent::PointerMotion(
            PointerMotionSample::relative(
                30,
                RelativeMotion {
                    dx: 4.0,
                    dy: 6.0,
                    dx_unaccelerated: 7.0,
                    dy_unaccelerated: 9.0,
                },
            )
        )]
    );
}

#[test]
fn native_pointer_motion_sample_keeps_relative_delta_when_cursor_clamps_at_edge() {
    let mut input = NativeInputState::new(320, 200);
    let sample = PointerMotionSample::relative(
        42,
        RelativeMotion {
            dx: 1_000.0,
            dy: -1_000.0,
            dx_unaccelerated: 1_200.0,
            dy_unaccelerated: -1_200.0,
        },
    );

    let effect = input.handle_hardware_input_event(NativeHardwareInputEvent::PointerMotion(sample));

    assert_eq!(effect.pointer_motion, Some((319.0, 0.0)));
    assert_eq!(effect.relative_motion, Some(sample.relative.unwrap()));
}

#[test]
fn native_input_coalescing_preserves_button_boundaries() {
    let events = coalesce_pointer_motion_events(vec![
        NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
            10,
            RelativeMotion::accelerated_only(1.0, 0.0),
        )),
        NativeHardwareInputEvent::PointerButton {
            button: u32::from(BTN_LEFT),
            pressed: true,
        },
        NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
            20,
            RelativeMotion::accelerated_only(0.0, 2.0),
        )),
    ]);

    assert_eq!(
        events,
        vec![
            NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
                10,
                RelativeMotion::accelerated_only(1.0, 0.0),
            )),
            NativeHardwareInputEvent::PointerButton {
                button: u32::from(BTN_LEFT),
                pressed: true,
            },
            NativeHardwareInputEvent::PointerMotion(PointerMotionSample::relative(
                20,
                RelativeMotion::accelerated_only(0.0, 2.0),
            )),
        ]
    );
}

#[test]
fn native_input_coalesces_consecutive_absolute_motion_to_latest_position() {
    let events = coalesce_pointer_motion_events(vec![
        NativeHardwareInputEvent::PointerMotion(PointerMotionSample::absolute(10, 12.0, 30.0)),
        NativeHardwareInputEvent::PointerMotion(PointerMotionSample::absolute(20, 18.0, 35.0)),
    ]);

    assert_eq!(
        events,
        vec![NativeHardwareInputEvent::PointerMotion(
            PointerMotionSample::absolute(20, 18.0, 35.0)
        )]
    );
}

#[test]
fn input_event_paths_select_only_real_keyboard_and_mouse_devices() {
    let root = std::env::current_dir()
        .unwrap()
        .join("target")
        .join("native-input-tests")
        .join(std::process::id().to_string());
    let dev_root = root.join("dev-input");
    let udev_root = root.join("udev-data");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&dev_root).unwrap();
    fs::create_dir_all(&udev_root).unwrap();
    fs::write(dev_root.join("event3"), "").unwrap();
    fs::write(dev_root.join("event4"), "").unwrap();
    fs::write(dev_root.join("event12"), "").unwrap();
    fs::write(udev_root.join("c13:67"), "E:ID_INPUT_MOUSE=1\n").unwrap();
    fs::write(udev_root.join("c13:68"), "E:ID_INPUT_KEYBOARD=1\n").unwrap();
    fs::write(udev_root.join("c13:76"), "E:ID_INPUT=1\n").unwrap();

    let paths = input_event_paths_with_udev(&dev_root, &udev_root);
    let names = paths
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    let _ = fs::remove_dir_all(&root);

    assert_eq!(names, ["event3", "event4"]);
}

#[test]
fn native_repaint_decision_skips_visible_frame_callback_without_damage() {
    assert_eq!(
        native_repaint_decision(NativeRepaintInputs {
            accepted_clients: false,
            render_generation_changed: false,
            pending_frame_work: true,
            only_pending_surface_frame_callbacks: true,
            redraw_requested: false,
            page_flip_pending: false,
        }),
        NativeRepaintDecision {
            repaint: false,
            protocol_only_present: true,
        }
    );
}

#[test]
fn native_repaint_decision_paints_non_callback_pending_frame_work() {
    assert_eq!(
        native_repaint_decision(NativeRepaintInputs {
            accepted_clients: false,
            render_generation_changed: false,
            pending_frame_work: true,
            only_pending_surface_frame_callbacks: false,
            redraw_requested: false,
            page_flip_pending: false,
        }),
        NativeRepaintDecision {
            repaint: true,
            protocol_only_present: false,
        }
    );
}

#[test]
fn native_repaint_decision_paints_visual_changes_even_with_frame_callback() {
    assert_eq!(
        native_repaint_decision(NativeRepaintInputs {
            accepted_clients: false,
            render_generation_changed: true,
            pending_frame_work: true,
            only_pending_surface_frame_callbacks: true,
            redraw_requested: false,
            page_flip_pending: false,
        }),
        NativeRepaintDecision {
            repaint: true,
            protocol_only_present: false,
        }
    );
}

#[test]
fn native_repaint_decision_waits_for_pending_pageflip_before_repaint() {
    assert_eq!(
        native_repaint_decision(NativeRepaintInputs {
            accepted_clients: false,
            render_generation_changed: true,
            pending_frame_work: true,
            only_pending_surface_frame_callbacks: false,
            redraw_requested: true,
            page_flip_pending: true,
        }),
        NativeRepaintDecision {
            repaint: false,
            protocol_only_present: false,
        }
    );
}
