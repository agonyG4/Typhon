use super::*;

    #[test]
    pub(crate) fn connected_connector_for_card_prefers_connected_matching_card_output() {
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
    pub(crate) fn matching_render_node_for_card_uses_same_drm_device_directory() {
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
    pub(crate) fn native_vrr_preference_parses_policy_values() {
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
    pub(crate) fn native_vrr_plan_auto_enables_only_when_connector_is_capable() {
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
    pub(crate) fn drm_mode_name_reads_nul_terminated_kernel_mode_name() {
        let mut mode = drm_sys::drm_mode_modeinfo::default();
        for (index, byte) in b"2560x1440\0ignored".iter().enumerate() {
            mode.name[index] = *byte as _;
        }

        assert_eq!(drm_mode_name(&mode), "2560x1440");
    }

    #[test]
    pub(crate) fn native_perf_log_env_accepts_truthy_values() {
        assert!(native_perf_log_value_enabled("1"));
        assert!(native_perf_log_value_enabled("true"));
        assert!(native_perf_log_value_enabled("debug"));
        assert!(!native_perf_log_value_enabled("0"));
        assert!(!native_perf_log_value_enabled("false"));
        assert!(!native_perf_log_value_enabled(""));
    }

    #[test]
    pub(crate) fn native_perf_line_formats_structured_fields() {
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
    pub(crate) fn native_app_gpu_preference_parses_explicit_values() {
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
    pub(crate) fn native_app_gpu_policy_resolves_from_active_scanout_backend() {
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
    pub(crate) fn native_launch_request_ignores_empty_command() {
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
    pub(crate) fn native_launch_request_preserves_args_policy_and_source() {
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
    pub(crate) fn native_runtime_error_includes_stage_backend_frame_and_recovery_command() {
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
    pub(crate) fn native_damage_accumulator_reports_full_surface_damage() {
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
    pub(crate) fn native_damage_accumulator_maps_partial_surface_damage_to_output() {
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
    pub(crate) fn native_damage_accumulator_maps_render_scene_element_damage_to_output() {
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
    pub(crate) fn native_damage_accumulator_clips_partial_surface_damage_to_output() {
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
    pub(crate) fn native_damage_summary_full_output_fallback_counts_output_pixels() {
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
    pub(crate) fn native_output_damage_for_window_move_covers_old_and_new_surface_bounds() {
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
    pub(crate) fn native_damage_accumulator_render_element_bounds_changes_cover_old_and_new_targets() {
        let previous = test_renderable_surface(7, 0, 0, 120, 80, RenderableSurfaceDamage::Full);
        let current = test_renderable_surface(7, 200, 100, 120, 80, RenderableSurfaceDamage::Full);
        let previous_elements =
            render_scene_elements_for_surfaces(std::slice::from_ref(&previous), 1.0);
        let current_elements =
            render_scene_elements_for_surfaces(std::slice::from_ref(&current), 1.0);

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
    pub(crate) fn native_output_damage_for_window_resize_covers_rescaled_bounds() {
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
    pub(crate) fn task_05_8_native_damage_for_window_resize_covers_visual_clip_changes() {
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
    pub(crate) fn native_output_damage_for_surface_commit_bounds_change_covers_old_and_new_bounds() {
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
    pub(crate) fn native_output_damage_forces_full_copy_after_full_scene_rebuild() {
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

    pub(crate) fn test_renderable_surface(
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
    pub(crate) fn proc_stat_cpu_parser_reads_user_and_system_ticks_after_comm() {
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
    pub(crate) fn kms_mode_preference_parses_exact_resolution_and_refresh() {
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
    pub(crate) fn kms_mode_selection_prefers_exact_refresh_when_available() {
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
    pub(crate) fn kms_mode_selection_uses_nearest_refresh_for_exact_resolution() {
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
    pub(crate) fn kms_mode_selection_highrr_prioritizes_refresh_then_resolution() {
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
    pub(crate) fn kms_mode_selection_auto_prioritizes_resolution_then_refresh() {
        let modes = [
            test_drm_mode(1920, 1080, 165),
            test_drm_mode(2560, 1440, 60),
            test_drm_mode(2560, 1440, 144),
        ];

        let selected = select_kms_mode(&modes, NativeModePreference::Auto).expect("mode selected");

        assert_eq!(mode_tuple(&selected), (2560, 1440, 144));
    }

    #[test]
    pub(crate) fn select_crtc_prefers_encoder_current_crtc_when_available() {
        let encoder = drm_sys::drm_mode_get_encoder {
            crtc_id: 42,
            possible_crtcs: 0b010,
            ..Default::default()
        };

        assert_eq!(select_crtc_id(&[12, 42, 77], &encoder), Some(42));
    }

    #[test]
    pub(crate) fn select_crtc_falls_back_to_possible_crtc_bitset() {
        let encoder = drm_sys::drm_mode_get_encoder {
            crtc_id: 0,
            possible_crtcs: 0b100,
            ..Default::default()
        };

        assert_eq!(select_crtc_id(&[12, 42, 77], &encoder), Some(77));
    }

    pub(crate) fn test_drm_mode(width: u16, height: u16, refresh_hz: u32) -> drm_sys::drm_mode_modeinfo {
        drm_sys::drm_mode_modeinfo {
            hdisplay: width,
            vdisplay: height,
            vrefresh: refresh_hz,
            ..Default::default()
        }
    }

    pub(crate) fn mode_tuple(mode: &drm_sys::drm_mode_modeinfo) -> (u16, u16, u32) {
        (mode.hdisplay, mode.vdisplay, mode.vrefresh)
    }

    #[test]
    pub(crate) fn native_drm_backend_plan_prefers_libseat_when_available() {
        let plan = NativeDrmBackendPlan::choose(NativeDrmBackendChoice {
            preference: NativeDrmBackendPreference::Auto,
            seat_available: true,
        });

        assert_eq!(plan.primary, NativeDrmBackendKind::Libseat);
        assert_eq!(plan.fallbacks, vec![NativeDrmBackendKind::Direct]);
    }

    #[test]
    pub(crate) fn native_drm_backend_plan_uses_direct_without_seat() {
        let plan = NativeDrmBackendPlan::choose(NativeDrmBackendChoice {
            preference: NativeDrmBackendPreference::Auto,
            seat_available: false,
        });

        assert_eq!(plan.primary, NativeDrmBackendKind::Direct);
        assert!(plan.fallbacks.is_empty());
    }

    #[test]
    pub(crate) fn native_drm_backend_plan_can_force_libseat() {
        let plan = NativeDrmBackendPlan::choose(NativeDrmBackendChoice {
            preference: NativeDrmBackendPreference::Libseat,
            seat_available: true,
        });

        assert_eq!(plan.primary, NativeDrmBackendKind::Libseat);
        assert!(plan.fallbacks.is_empty());
    }

    #[test]
    pub(crate) fn native_drm_backend_plan_rejects_forced_libseat_without_seat() {
        let plan = NativeDrmBackendPlan::choose(NativeDrmBackendChoice {
            preference: NativeDrmBackendPreference::Libseat,
            seat_available: false,
        });

        assert_eq!(plan.primary, NativeDrmBackendKind::Unavailable);
        assert!(plan.fallbacks.is_empty());
    }

    #[test]
    pub(crate) fn native_scanout_plan_prefers_native_egl_gbm_when_ready() {
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
    pub(crate) fn native_scanout_plan_can_force_gpu_without_cpu_fallback() {
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
    pub(crate) fn native_scanout_plan_rejects_forced_gpu_without_egl() {
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
    pub(crate) fn native_scanout_plan_fallback_after_gpu_failure_preserves_remaining_candidates() {
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
    pub(crate) fn native_scanout_kind_names_cpu_write_gbm_backend_honestly() {
        assert_eq!(
            NativeScanoutKind::GbmCpuWritePageFlip.as_str(),
            "GBM CPU-write pageflip"
        );
    }

    #[test]
    pub(crate) fn injected_native_egl_gbm_open_failure_returns_clear_error_before_kms_use() {
        let previous = std::env::var_os("OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM");
        unsafe {
            std::env::set_var("OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM", "1");
        }
        let file = fs::File::open("Cargo.toml").unwrap();

        let error = match NativeScanoutBackend::open_kind(
            NativeScanoutKind::NativeEglGbm,
            &file,
            1,
            1,
            1,
        ) {
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
    pub(crate) fn auto_gpu_open_failure_next_cpu_candidate_resolves_apps_to_cpu() {
        let plan = NativeScanoutPlan::choose(NativeScanoutChoice {
            preference: NativeScanoutPreference::Auto,
            gbm_available: true,
            egl_available: true,
            page_flip_available: true,
        });

        let fallback = plan.after_failed(NativeScanoutKind::NativeEglGbm);

        assert_eq!(fallback.primary, NativeScanoutKind::GbmCpuWritePageFlip);
        assert_eq!(
            resolve_native_app_gpu_policy(CompositorAppGpuPreference::Auto, fallback.primary)
                .unwrap(),
            EffectiveCompositorAppGpuPolicy::CpuOnly
        );
    }

    #[test]
    pub(crate) fn native_scanout_preference_keeps_legacy_gbm_egl_alias_for_cpu_write_backend() {
        assert_eq!(
            NativeScanoutPreference::parse("gbm-egl"),
            NativeScanoutPreference::GbmCpuWritePageFlip
        );
    }

    #[test]
    pub(crate) fn native_scanout_preference_accepts_canonical_cpu_write_backend_name() {
        assert_eq!(
            NativeScanoutPreference::parse("gbm-cpu-write"),
            NativeScanoutPreference::GbmCpuWritePageFlip
        );
    }

    #[test]
    pub(crate) fn native_scanout_preference_accepts_canonical_gpu_backend_name() {
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
    pub(crate) fn native_scanout_plan_uses_cpu_gbm_fallback_without_egl() {
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
    pub(crate) fn native_pageflip_state_blocks_overlapping_flips() {
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
    pub(crate) fn native_pageflip_state_rejects_mismatch_without_clearing_pending() {
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
    pub(crate) fn native_pageflip_state_stale_event_cannot_complete_new_submission() {
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
    pub(crate) fn native_pageflip_token_wrap_skips_zero() {
        assert_eq!(next_nonzero_page_flip_token(u64::MAX), 1);
        assert_eq!(next_nonzero_page_flip_token(1), 2);
    }

    #[test]
    pub(crate) fn native_pageflip_token_does_not_restart_after_backend_recreation() {
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
    pub(crate) fn native_pageflip_buffers_promote_ready_to_pending_to_current() {
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
    pub(crate) fn native_pageflip_buffers_finish_initial_scanout_promotes_ready() {
        let mut buffers = NativePageFlipBuffers::default();

        buffers.set_ready(20);
        buffers.finish_initial_scanout();

        assert_eq!(buffers.ready_or_current(), Some(&20));
        assert_eq!(buffers.take_ready(), None);
    }

    #[test]
    pub(crate) fn native_initial_frame_paints_real_topbar() {
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
    pub(crate) fn native_initial_frame_does_not_draw_closed_spotlight_panel() {
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
